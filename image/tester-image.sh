#!/bin/bash
# Teste une image Fabrique-Clé dans QEMU, comme une vraie clé USB sur un PC inconnu :
# OVMF Secure Boot (clés Microsoft), NVRAM vierge, disque USB plus grand que l'image.
# Le premier démarrage est piloté par un config.txt de test (chiffrement compris), puis
# l'image testée est inspectée hors ligne.
#
#   sudo ./tester-image.sh out/fabrique-cle.img
#
# L'image d'origine n'est jamais modifiée (le test travaille sur une copie creuse).
set -euo pipefail

ICI=$(dirname "$(readlink -f "$0")")
IMG=$(realpath -m "${1:-$ICI/out/fabrique-cle.img}")
CLE_TMP="${IMG%.img}.key"
TRAV=$ICI/out/test
EN_PLUS=${EN_PLUS:-6G}          # la « clé » de test est plus grande que l'image
DELAI=${DELAI:-1800}             # secondes avant de déclarer l'échec
PHRASE=fabrique-test-$RANDOM$RANDOM
VG=fabcle
CRYPT=fabcle_test

[ "$(id -u)" -eq 0 ] || { echo "Lancer avec sudo."; exit 1; }
journal() { printf '\n=== %s ===\n' "$*"; }

CODE= VARS=
for c in /usr/share/OVMF/OVMF_CODE_4M.ms.fd /usr/share/OVMF/OVMF_CODE_4M.secboot.fd; do [ -f "$c" ] && { CODE=$c; break; }; done
VARS=/usr/share/OVMF/OVMF_VARS_4M.ms.fd
[ -n "$CODE" ] && [ -f "$VARS" ] || { echo "OVMF Secure Boot introuvable (paquet ovmf)"; exit 1; }

MNT=$(mktemp -d)
LOOP=
nettoyer() {
  set +e
  umount -R "$MNT" 2>/dev/null
  vgchange -an "$VG" >/dev/null 2>&1
  cryptsetup close "$CRYPT" 2>/dev/null
  [ -n "$LOOP" ] && losetup -d "$LOOP"
  rmdir "$MNT"
}
trap nettoyer EXIT

attacher() { LOOP=$(losetup --show -fP "$TEST"); udevadm settle || true; partx -u "$LOOP" 2>/dev/null || true; }
detacher() { losetup -d "$LOOP"; LOOP=; }

# ---------------------------------------------------------------------------
journal "Copie de test (+$EN_PLUS) et config.txt"
rm -rf "$TRAV"; mkdir -p "$TRAV"
TEST=$TRAV/cle.img
cp --sparse=always "$IMG" "$TEST"
truncate -s "+$EN_PLUS" "$TEST"
TAILLE_AVANT=$(sgdisk -i 3 "$IMG" | awk -F'[()]' '/Partition size/ {print $1}' | awk '{print $3}')

attacher
mount "${LOOP}p1" "$MNT"
cat > "$MNT/fabrique-cle/config.txt" <<EOF
identifiant=eleve
nom_complet=Élève Test
mot_de_passe_hache=$(printf 'test1234' | openssl passwd -6 -stdin)
machine=cle-test
clavier=be
langue=fr_BE.UTF-8
chiffrer=oui
phrase_luks_test=$PHRASE
eteindre_apres=oui
EOF
umount "$MNT"
detacher
cp "$VARS" "$TRAV/vars.fd"

# ---------------------------------------------------------------------------
journal "Premier démarrage dans QEMU (Secure Boot, clé USB, délai ${DELAI}s)"
KVM=()
[ -w /dev/kvm ] && KVM=(-enable-kvm -cpu host) || echo "ATTENTION : pas de KVM, ce sera très lent"
rc=0
timeout "$DELAI" qemu-system-x86_64 "${KVM[@]}" \
  -machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on \
  -m 3072 -smp 2 \
  -drive if=pflash,format=raw,unit=0,file="$CODE",readonly=on \
  -drive if=pflash,format=raw,unit=1,file="$TRAV/vars.fd" \
  -device qemu-xhci -drive if=none,id=cle,format=raw,file="$TEST" -device usb-storage,drive=cle,removable=on \
  -nic user,model=virtio-net-pci \
  -display none -monitor unix:"$TRAV/monitor.sock",server,nowait \
  -serial file:"$TRAV/serie.log" &
QEMU=$!
# Captures d'écran régulières pour diagnostiquer un échec
( while kill -0 $QEMU 2>/dev/null; do
    sleep 20
    echo "screendump $TRAV/ecran.ppm" | socat - UNIX-CONNECT:"$TRAV/monitor.sock" >/dev/null 2>&1 || true
  done ) &
wait $QEMU || rc=$?
[ "$rc" -eq 0 ] || { echo "ÉCHEC : QEMU code $rc (124 = délai dépassé ; voir $TRAV/ecran.ppm)"; exit 1; }

# ---------------------------------------------------------------------------
journal "Inspection de la clé après le premier démarrage"
attacher
P3=${LOOP}p3
ERR=0
verifier() { if (eval "$2"); then echo "OK    $1"; else echo "ÉCHEC $1"; ERR=1; fi; }

verifier "config.txt effacé de l'ESP" "mount -o ro ${LOOP}p1 $MNT && [ ! -e $MNT/fabrique-cle/config.txt ]; r=\$?; umount $MNT; exit \$r"
verifier "pas de fbx64.efi dans EFI/BOOT" "mount -o ro ${LOOP}p1 $MNT && [ ! -e $MNT/EFI/BOOT/fbx64.efi ]; r=\$?; umount $MNT; exit \$r"
verifier "clé temporaire de l'image refusée" "! cryptsetup open --test-passphrase --key-file '$CLE_TMP' $P3"
verifier "une seule phrase (celle de l'élève)" "[ \$(cryptsetup luksDump $P3 | grep -cE '^ +[0-9]+: luks2') -eq 1 ]"
printf '%s' "$PHRASE" | cryptsetup open --key-file - "$P3" "$CRYPT"
vgchange -ay "$VG" >/dev/null
mount -o ro "/dev/$VG/root" "$MNT"
echo "--- journal du premier démarrage ---"; cat "$MNT/var/log/fabrique-cle.log"; echo "---"
verifier "premier démarrage terminé" "grep -q TERMINÉ $MNT/var/log/fabrique-cle.log"
verifier "démarré avec Secure Boot actif" "grep -q 'Secure Boot : 1' $MNT/var/log/fabrique-cle.log"
verifier "compte eleve créé, membre de sudo" "grep -q '^eleve:' $MNT/etc/passwd && grep -qE '^sudo:.*\\beleve\\b' $MNT/etc/group"
verifier "nom de machine cle-test" "[ \"\$(cat $MNT/etc/hostname)\" = cle-test ]"
verifier "crypttab sans clé temporaire" "grep -qE '^fabcle_crypt\\s+\\S+\\s+none\\s' $MNT/etc/crypttab"
verifier "plus de fichier de clé temporaire" "[ ! -e $MNT/etc/cryptsetup-keys.d/fabcle_crypt.key ]"
verifier "machine-id générée au démarrage" "[ -s $MNT/etc/machine-id ]"
verifier "état de premier démarrage effacé" "[ ! -e $MNT/var/lib/fabrique-cle/a-configurer ]"
umount "$MNT"
vgchange -an "$VG" >/dev/null; cryptsetup close "$CRYPT"
TAILLE_APRES=$(sgdisk -i 3 "$LOOP" | awk -F'[()]' '/Partition size/ {print $1}' | awk '{print $3}')
verifier "partition agrandie ($TAILLE_AVANT -> $TAILLE_APRES secteurs)" "[ $TAILLE_APRES -gt $TAILLE_AVANT ]"
detacher

[ "$ERR" -eq 0 ] && journal "TEST RÉUSSI" || { journal "TEST ÉCHOUÉ"; exit 1; }
