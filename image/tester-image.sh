#!/bin/bash
# Teste une image Fabrique-Clé dans QEMU, comme une vraie clé USB sur un PC inconnu :
# OVMF Secure Boot (clés Microsoft), NVRAM vierge, disque USB plus grand que l'image.
# Le premier démarrage est piloté par un config.txt de test (chiffrement compris) ; la phrase
# secrète est tapée au clavier virtuel, comme le ferait l'utilisateur, une fois la question réellement
# visible à l'écran. L'image testée est ensuite inspectée hors ligne.
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
# Phrase tapée au clavier : uniquement des lettres placées au même endroit en AZERTY et en QWERTY
# (le clavier belge est actif quand la question est posée).
PHRASE=$(head -c 4096 /dev/urandom | LC_ALL=C tr -dc 'bcdefghijklnoprstuvxy' | head -c 16)
ATTENTE_QUESTION=${ATTENTE_QUESTION:-900}   # secondes pour voir apparaître la question
VG=fabcle
CRYPT=fabcle_test

[ "$(id -u)" -eq 0 ] || { echo "Lancer avec sudo."; exit 1; }
# Espace de montage privé : nos montages ne se propagent pas aux services de l'hôte
# (qui les garderaient ouverts) et disparaissent avec le script
[ -n "${FC_PRIVE:-}" ] || exec env FC_PRIVE=1 unshare --mount --propagation private "$0" "$@"
vgs "$VG" >/dev/null 2>&1 && { echo "ABANDON : le groupe de volumes $VG est déjà actif sur cet hôte"; exit 1; }
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

# ---------------------------------------------------------------------------
# Saisie de la phrase, comme l'utilisateur : seulement quand la question est visible à l'écran.
moniteur() {  # commande pour le moniteur QEMU (réessaie si la capture périodique l'occupe)
  local i
  for i in 1 2 3 4 5; do
    echo "$1" | socat - UNIX-CONNECT:"$TRAV/monitor.sock" >/dev/null 2>&1 && return 0
    sleep 0.3
  done
  return 1
}
taper() {  # taper <texte> : lettres une à une, puis Entrée
  local i
  for ((i = 0; i < ${#1}; i++)); do moniteur "sendkey ${1:i:1}"; sleep 0.15; done
  moniteur "sendkey ret"
}
question_visible() {  # vrai si l'écran est majoritairement le fond bleu de whiptail
  moniteur "screendump $TRAV/question.ppm" || return 1
  sleep 1
  python3 - "$TRAV/question.ppm" <<'PY'
import sys
d = open(sys.argv[1], "rb").read()
entete = d.split(b"\n", 3)          # P6 / largeur hauteur / 255 / pixels
l, h = map(int, entete[1].split())
px = entete[3]
pas = 3 * 7
n = len(range(0, len(px) - 2, pas))
bleus = sum(1 for i in range(0, len(px) - 2, pas) if px[i + 2] > 120 and px[i] < 60 and px[i + 1] < 60)
sys.exit(0 if n and bleus / n > 0.4 else 1)
PY
}
( debut=$SECONDS
  until question_visible; do
    if ! kill -0 $QEMU 2>/dev/null; then exit 0; fi
    if (( SECONDS - debut > ATTENTE_QUESTION )); then
      echo "ÉCHEC : question de la phrase secrète jamais visible à l'écran (voir $TRAV/question.ppm)"
      cp "$TRAV/question.ppm" "$TRAV/ecran.ppm" 2>/dev/null || true
      kill $QEMU; exit 1
    fi
    sleep 5
  done
  echo "Question visible après $((SECONDS - debut)) s : saisie de la phrase (deux fois)"
  taper "$PHRASE"; sleep 3; taper "$PHRASE"
  # Capture pendant le chiffrement (≈ 1 min 40 dans QEMU) : la jauge doit être à l'écran (artefact CI)
  sleep 30; moniteur "screendump $TRAV/jauge.ppm" || true
) &
wait $QEMU || rc=$?
if [ "$rc" -ne 0 ]; then echo "ÉCHEC : QEMU code $rc (124 = délai dépassé ; voir $TRAV/ecran.ppm)"; fi

# Journal du premier démarrage (clé temporaire ou phrase de test, selon l'étape atteinte)
attacher
if cryptsetup open --key-file "$CLE_TMP" "${LOOP}p3" "$CRYPT" 2>/dev/null || \
   printf '%s' "$PHRASE" | cryptsetup open --key-file - "${LOOP}p3" "$CRYPT" 2>/dev/null; then
  if vgchange -ay "$VG" >/dev/null && mount -o ro "/dev/$VG/root" "$MNT"; then
    echo "--- journal du premier démarrage ---"; cat "$MNT/var/log/fabrique-cle.log" || true; echo "---"
    cp "$MNT/var/log/fabrique-cle.log" "$TRAV/" 2>/dev/null || true
    umount "$MNT"
  fi
  vgchange -an "$VG" >/dev/null || true; cryptsetup close "$CRYPT" || true
fi
detacher
[ "$rc" -eq 0 ] || exit 1

# ---------------------------------------------------------------------------
journal "Deuxième démarrage : phrase LUKS au clavier, jusqu'à l'écran de connexion"
# Vérifie ce que le premier démarrage ne voit pas : la question LUKS (initramfs allégé, clavier
# belge) puis le démarrage complet jusqu'à SDDM, avec la phrase tapée au clavier virtuel.
cat > "$TRAV/ecran.py" <<'PY'
# ecran.py capture.ppm [précédente.ppm] -> « sombre couleurs différence »
import sys
def lire(f):
    d = open(f, "rb").read().split(b"\n", 3)
    return d[3]
px = lire(sys.argv[1])
pas = 3 * 11
idx = range(0, len(px) - 2, pas)
sombres = sum(1 for i in idx if px[i] < 40 and px[i + 1] < 40 and px[i + 2] < 40) / max(1, len(idx))
couleurs = len({(px[i] >> 4, px[i + 1] >> 4, px[i + 2] >> 4) for i in idx})
diff = 1.0
if len(sys.argv) > 2:
    try:
        q = lire(sys.argv[2])
        if len(q) == len(px):
            diff = sum(1 for i in idx if px[i:i + 3] != q[i:i + 3]) / max(1, len(idx))
    except OSError:
        pass
print(f"{sombres:.3f} {couleurs} {diff:.4f}")
PY
capture() { moniteur "screendump $1" && sleep 1 && python3 "$TRAV/ecran.py" "$1" "${2:-}"; }
rm -f "$TRAV/monitor.sock" "$TRAV"/e2-*.ppm
timeout 900 qemu-system-x86_64 "${KVM[@]}" \
  -machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on \
  -m 3072 -smp 2 \
  -drive if=pflash,format=raw,unit=0,file="$CODE",readonly=on \
  -drive if=pflash,format=raw,unit=1,file="$TRAV/vars.fd" \
  -device qemu-xhci -drive if=none,id=cle,format=raw,file="$TEST" -device usb-storage,drive=cle,removable=on \
  -nic user,model=virtio-net-pci \
  -display none -monitor unix:"$TRAV/monitor.sock",server,nowait \
  -serial file:"$TRAV/serie2.log" &
QEMU=$!
sleep 15
etat=attente; essais=0; debut=$SECONDS; precedent=
while kill -0 $QEMU 2>/dev/null && (( SECONDS - debut < 600 )); do
  sleep 5
  f="$TRAV/e2-$SECONDS.ppm"
  read -r sombre couleurs diff < <(capture "$f" "$precedent" || echo "1 0 1")
  precedent=$f
  # Écran de connexion : image en couleurs (fond Plasma), plus un écran de texte
  if [ "$etat" = saisie ] && awk "BEGIN{exit !($sombre < 0.5 && $couleurs > 40)}"; then
    echo "Écran de connexion atteint $((SECONDS - debut)) s après la saisie (sombre=$sombre couleurs=$couleurs)"
    cp "$f" "$TRAV/connexion.ppm"; etat=connexion; break
  fi
  # Question LUKS : écran sombre, avec un peu de texte, stable entre deux captures
  if awk "BEGIN{exit !($sombre > 0.9 && $sombre < 0.9999 && $diff < 0.002)}"; then
    if [ "$etat" = attente ] || { [ "$etat" = saisie ] && (( SECONDS - t_saisie > 90 && essais < 2 )); }; then
      essais=$((essais + 1)); t_saisie=$SECONDS
      echo "Question LUKS (écran stable, sombre=$sombre) : saisie de la phrase, essai $essais"
      cp "$f" "$TRAV/question-luks.ppm"
      taper "$PHRASE"; etat=saisie
    fi
  fi
done
if [ "$etat" != connexion ]; then
  echo "ÉCHEC : écran de connexion jamais atteint au deuxième démarrage (état : $etat, voir ecran.ppm)"
  cp "${precedent:-/dev/null}" "$TRAV/ecran.ppm" 2>/dev/null || true
  kill $QEMU 2>/dev/null; wait $QEMU 2>/dev/null; exit 1
fi
# Arrêt propre (bouton d'alimentation ACPI), sinon coupure au bout de 2 min
moniteur system_powerdown || true
for _ in $(seq 24); do kill -0 $QEMU 2>/dev/null || break; sleep 5; done
kill -0 $QEMU 2>/dev/null && { echo "Arrêt ACPI trop long : coupure"; moniteur quit || kill $QEMU; }
wait $QEMU 2>/dev/null || true

# ---------------------------------------------------------------------------
journal "Inspection de la clé après le premier démarrage"
attacher
P3=${LOOP}p3
ERR=0
verifier() { if (eval "$2"); then echo "OK    $1"; else echo "ÉCHEC $1"; ERR=1; fi; }

verifier "config.txt effacé de l'ESP" "mount -o ro ${LOOP}p1 $MNT && [ ! -e $MNT/fabrique-cle/config.txt ]; r=\$?; umount $MNT; exit \$r"
verifier "pas de fbx64.efi dans EFI/BOOT" "mount -o ro ${LOOP}p1 $MNT && [ ! -e $MNT/EFI/BOOT/fbx64.efi ]; r=\$?; umount $MNT; exit \$r"
verifier "clé temporaire de l'image refusée" "! cryptsetup open --test-passphrase --key-file '$CLE_TMP' $P3"
verifier "volume chiffré en AES-XTS, plus de cipher_null" "d=\$(cryptsetup luksDump $P3); grep -qE '^\s+cipher:\s+aes-xts-plain64' <<<\"\$d\" && ! grep -q cipher_null <<<\"\$d\""
verifier "une seule phrase (celle de l'utilisateur)" "[ \$(cryptsetup luksDump $P3 | grep -cE '^ +[0-9]+: luks2') -eq 1 ]"
printf '%s' "$PHRASE" | cryptsetup open --key-file - "$P3" "$CRYPT"
vgchange -ay "$VG" >/dev/null
mount -o ro "/dev/$VG/root" "$MNT"
verifier "premier démarrage terminé" "grep -q TERMINÉ $MNT/var/log/fabrique-cle.log"
verifier "progression du chiffrement suivie jusqu'à 100 % (jauge)" "grep -qE 'chiffrement 100 % \\(' $MNT/var/log/fabrique-cle.log"
verifier "démarré avec Secure Boot actif" "grep -q 'Secure Boot : 1' $MNT/var/log/fabrique-cle.log"
verifier "compte eleve créé, membre de sudo" "grep -q '^eleve:' $MNT/etc/passwd && grep -qE '^sudo:.*\\beleve\\b' $MNT/etc/group"
verifier "nom de machine cle-test" "[ \"\$(cat $MNT/etc/hostname)\" = cle-test ]"
verifier "crypttab sans clé temporaire" "grep -qE '^fabcle_crypt\\s+\\S+\\s+none\\s' $MNT/etc/crypttab"
verifier "plus de fichier de clé temporaire" "[ ! -e $MNT/etc/cryptsetup-keys.d/fabcle_crypt.key ]"
verifier "machine-id générée au démarrage" "[ -s $MNT/etc/machine-id ]"
verifier "synchronisation de l'heure (systemd-timesyncd) activée" "[ -L $MNT/etc/systemd/system/sysinit.target.wants/systemd-timesyncd.service ]"
installe() { grep -A1 -x "Package: $1" $MNT/var/lib/dpkg/status | grep -q 'Status: install ok installed'; }
verifier "Brave installé, seul navigateur (ni Firefox ni Konqueror)" "$(declare -f installe); installe brave-browser && ! installe firefox-esr && ! installe konqueror && ! installe chromium"
verifier "Brave navigateur par défaut (mimeapps, Plasma)" "grep -q 'x-scheme-handler/https=brave-browser.desktop' $MNT/etc/xdg/mimeapps.list && grep -q 'BrowserApplication=brave-browser.desktop' $MNT/etc/xdg/kdeglobals"
verifier "fond « image du jour » (plugin + lancement à la 1re session)" "$(declare -f installe); installe plasma-wallpapers-addons && [ -x $MNT/usr/local/bin/fabrique-cle-fond-du-jour ] && [ -e $MNT/etc/xdg/autostart/fabrique-cle-fond-du-jour.desktop ]"
verifier "curl installé" "$(declare -f installe); installe curl"
verifier "réglages clé USB : sysctl, BFQ (udev 61 + module), journal" "[ -e $MNT/etc/sysctl.d/90-fabrique-cle.conf ] && grep -q 'ID_BUS}==\"usb\".*bfq' $MNT/etc/udev/rules.d/61-fabrique-cle-usb.rules && grep -qx bfq $MNT/etc/modules-load.d/fabrique-cle-bfq.conf && grep -q 'SyncIntervalSec=30s' $MNT/etc/systemd/journald.conf.d/fabrique-cle.conf"
verifier "racine en noatime,commit=30 ; ~/.cache de eleve en RAM" "grep -qE '\\s/\\s+ext4\\s+\\S*noatime,commit=30' $MNT/etc/fstab && grep -qE '^tmpfs /home/eleve/.cache tmpfs ' $MNT/etc/fstab"
verifier "aucune mise à jour automatique (apt-daily masqués), pas d'attente du réseau" "[ \"\$(readlink $MNT/etc/systemd/system/apt-daily.timer)\" = /dev/null ] && [ \"\$(readlink $MNT/etc/systemd/system/apt-daily-upgrade.timer)\" = /dev/null ] && [ ! -L $MNT/etc/systemd/system/network-online.target.wants/NetworkManager-wait-online.service ]"
verifier "menu GRUB à 1 s" "grep -qx 'GRUB_TIMEOUT=1' $MNT/etc/default/grub"
verifier "état de premier démarrage effacé" "[ ! -e $MNT/var/lib/fabrique-cle/a-configurer ]"
umount "$MNT"
vgchange -an "$VG" >/dev/null; cryptsetup close "$CRYPT"
TAILLE_APRES=$(sgdisk -i 3 "$LOOP" | awk -F'[()]' '/Partition size/ {print $1}' | awk '{print $3}')
verifier "partition agrandie ($TAILLE_AVANT -> $TAILLE_APRES secteurs)" "[ $TAILLE_APRES -gt $TAILLE_AVANT ]"
detacher

[ "$ERR" -eq 0 ] && journal "TEST RÉUSSI" || { journal "TEST ÉCHOUÉ"; exit 1; }
