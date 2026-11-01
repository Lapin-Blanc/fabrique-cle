#!/bin/bash
# Construit l'image système Fabrique-Clé (Debian 13 « Trixie » + KDE Plasma minimal) dans un fichier.
#
#   sudo ./construire-image.sh [sortie.img]
#
# Hôte : Debian/Ubuntu avec mmdebstrap, gdisk, cryptsetup, lvm2, dosfstools (root, périphériques loop).
# Ne touche à aucun disque réel : tout se passe dans un fichier image monté en loop.
#
# Disposition (identique à la clé SSK validée) :
#   p1 ESP 512 Mio FAT32 « FABCLE »  |  p2 /boot 1 Gio ext4  |  p3 LUKS2 -> LVM fabcle/root ext4
# Le volume LUKS est ouvert par une clé temporaire incluse dans l'initramfs : le service de premier
# démarrage la remplace par la phrase de l'élève et rechiffre le volume (la clé maîtresse de l'image
# publiée est publique).
set -euo pipefail

ICI=$(dirname "$(readlink -f "$0")")
SORTIE=$(realpath -m "${1:-$ICI/out/fabrique-cle.img}")
TAILLE=${TAILLE:-10G}
SUITE=trixie
MIROIR=${MIROIR:-http://deb.debian.org/debian}
COMPOSANTS="main contrib non-free non-free-firmware"
VG=fabcle
CRYPT=fabcle_crypt
CLE_CIBLE=/etc/cryptsetup-keys.d/$CRYPT.key
VERSION=${VERSION:-dev}

[ "$(id -u)" -eq 0 ] || { echo "Lancer avec sudo."; exit 1; }
for o in mmdebstrap sgdisk cryptsetup pvcreate mkfs.vfat mkfs.ext4 partx; do
  command -v "$o" >/dev/null || { echo "Outil manquant : $o"; exit 1; }
done

journal() { printf '\n=== %s ===\n' "$*"; }

MNT=$(mktemp -d)
LOOP=
# Processus lancés dans le chroot par les scripts des paquets (ils bloqueraient le démontage)
tuer_chroot() {
  local p
  for p in /proc/[0-9]*; do
    [ "$(readlink "$p/root" 2>/dev/null)" = "$MNT" ] && { echo "arrêt du processus ${p#/proc/} ($(cat "$p/comm"))"; kill -9 "${p#/proc/}"; }
  done
  sleep 1
}
demonter() {
  tuer_chroot
  umount -R "$MNT"
  vgchange -an "$VG" >/dev/null
  cryptsetup close "$CRYPT"
  losetup -d "$LOOP"; LOOP=
}
nettoyer() {
  set +e
  if mountpoint -q "$MNT"; then tuer_chroot; umount -R "$MNT"; fi
  vgchange -an "$VG" >/dev/null 2>&1
  cryptsetup close "$CRYPT" 2>/dev/null
  [ -n "$LOOP" ] && losetup -d "$LOOP"
  rmdir "$MNT" 2>/dev/null
}
trap nettoyer EXIT

# ---------------------------------------------------------------------------
journal "Fichier image $SORTIE ($TAILLE) et partitions"
mkdir -p "$(dirname "$SORTIE")"
rm -f "$SORTIE"
truncate -s "$TAILLE" "$SORTIE"
LOOP=$(losetup --show -f "$SORTIE")
sgdisk -n1:1MiB:+512MiB -t1:EF00 -c1:EFI \
       -n2:0:+1GiB      -t2:8300 -c2:boot \
       -n3:0:0          -t3:8309 -c3:systeme "$LOOP"
losetup -d "$LOOP"
LOOP=$(losetup --show -fP "$SORTIE")
udevadm settle || true
for i in 1 2 3; do [ -b "${LOOP}p$i" ] || partx -a "$LOOP"; done
P1=${LOOP}p1 P2=${LOOP}p2 P3=${LOOP}p3

journal "LUKS2 (clé temporaire) -> LVM -> systèmes de fichiers"
CLE_TMP="${SORTIE%.img}.key"
head -c 64 /dev/urandom > "$CLE_TMP"
chmod 600 "$CLE_TMP"
cryptsetup luksFormat --batch-mode --type luks2 --label fabcle --key-file "$CLE_TMP" "$P3"
cryptsetup open --allow-discards --key-file "$CLE_TMP" "$P3" "$CRYPT"
pvcreate -q "/dev/mapper/$CRYPT"
vgcreate -q "$VG" "/dev/mapper/$CRYPT"
lvcreate -q -l 100%FREE -n root "$VG"
mkfs.vfat -F 32 -n FABCLE "$P1" >/dev/null
mkfs.ext4 -q -L boot "$P2"
mkfs.ext4 -q -L racine "/dev/$VG/root"

UUID_ESP=$(blkid -s UUID -o value "$P1")
UUID_BOOT=$(blkid -s UUID -o value "$P2")
UUID_LUKS=$(cryptsetup luksUUID "$P3")

mount "/dev/$VG/root" "$MNT"
rmdir "$MNT/lost+found"

# ---------------------------------------------------------------------------
journal "Système de base (mmdebstrap)"
mmdebstrap --variant=important --components="$COMPOSANTS" \
  --include=locales,console-setup,keyboard-configuration,ca-certificates \
  "$SUITE" "$MNT" "$MIROIR"

mkdir -p "$MNT/lost+found"
mv "$MNT/boot" "$MNT/boot.mmdebstrap" && mkdir "$MNT/boot"
mount "$P2" "$MNT/boot"
cp -a "$MNT/boot.mmdebstrap/." "$MNT/boot/" && rm -rf "$MNT/boot.mmdebstrap" "$MNT/boot/lost+found"
mkdir -p "$MNT/boot/efi"
mount "$P1" "$MNT/boot/efi"
mount --rbind /dev "$MNT/dev" && mount --make-rslave "$MNT/dev"
mount -t proc proc "$MNT/proc"
mount -t sysfs sys "$MNT/sys"
mount -t tmpfs tmpfs "$MNT/run"
cp -L /etc/resolv.conf "$MNT/etc/resolv.conf"

# ---------------------------------------------------------------------------
journal "Configuration (avant les paquets, pour que les hooks la prennent en compte)"
# Aucun service ne démarre pendant la construction
printf '#!/bin/sh\nexit 101\n' > "$MNT/usr/sbin/policy-rc.d"
chmod 755 "$MNT/usr/sbin/policy-rc.d"

cat > "$MNT/etc/apt/sources.list" <<EOF
deb http://deb.debian.org/debian $SUITE $COMPOSANTS
deb http://deb.debian.org/debian $SUITE-updates $COMPOSANTS
deb http://security.debian.org/debian-security $SUITE-security $COMPOSANTS
EOF
# Backports : NotAutomatic -> priorité 100, jamais choisis sans -t trixie-backports
echo "deb http://deb.debian.org/debian $SUITE-backports $COMPOSANTS" > "$MNT/etc/apt/sources.list.d/backports.list"
rm -f "$MNT/etc/apt/sources.list.d/debian.sources"

cat > "$MNT/etc/fstab" <<EOF
# <système de fichiers>                 <point de montage> <type> <options>                 <dump> <pass>
/dev/mapper/$VG-root                    /                  ext4   errors=remount-ro         0      1
UUID=$UUID_BOOT /boot              ext4   defaults                  0      2
UUID=$UUID_ESP                          /boot/efi          vfat   umask=0077                0      1
EOF
# Option initramfs : le hook ne peut pas deviner la racine depuis un chroot
echo "$CRYPT UUID=$UUID_LUKS $CLE_CIBLE luks,discard,initramfs" > "$MNT/etc/crypttab"
install -D -m 400 "$CLE_TMP" "$MNT$CLE_CIBLE"
chmod 700 "$MNT/etc/cryptsetup-keys.d"
mkdir -p "$MNT/etc/cryptsetup-initramfs"
cat > "$MNT/etc/cryptsetup-initramfs/conf-hook" <<'EOF'
# Clé temporaire de l'image, retirée par le service de premier démarrage
KEYFILE_PATTERN="/etc/cryptsetup-keys.d/*.key"
UMASK=0077
EOF

mkdir -p "$MNT/etc/initramfs-tools/conf.d"
cat > "$MNT/etc/initramfs-tools/conf.d/fabrique-cle.conf" <<'EOF'
# Clé nomade : tous les pilotes (matériel inconnu), clavier pour la phrase LUKS, pas d'hibernation
MODULES=most
KEYMAP=y
RESUME=none
EOF

cat > "$MNT/etc/default/keyboard" <<'EOF'
XKBMODEL="pc105"
XKBLAYOUT="be"
XKBVARIANT=""
XKBOPTIONS=""
BACKSPACE="guess"
EOF
cat > "$MNT/etc/locale.gen" <<'EOF'
fr_BE.UTF-8 UTF-8
fr_FR.UTF-8 UTF-8
fr_CH.UTF-8 UTF-8
nl_BE.UTF-8 UTF-8
en_US.UTF-8 UTF-8
EOF
echo 'LANG=fr_BE.UTF-8' > "$MNT/etc/default/locale"
ln -sf /usr/share/zoneinfo/Europe/Brussels "$MNT/etc/localtime"
echo Europe/Brussels > "$MNT/etc/timezone"
# Horloge matérielle en heure locale (cohabitation Windows)
printf '0.0 0 0.0\n0\nLOCAL\n' > "$MNT/etc/adjtime"
echo fabrique-cle > "$MNT/etc/hostname"
echo "$VERSION" > "$MNT/etc/fabrique-cle-version"
cat > "$MNT/etc/hosts" <<'EOF'
127.0.0.1	localhost
127.0.1.1	fabrique-cle
::1		localhost ip6-localhost ip6-loopback
EOF
# NetworkManager gère toutes les interfaces
cat > "$MNT/etc/network/interfaces" <<'EOF'
source /etc/network/interfaces.d/*
auto lo
iface lo inet loopback
EOF

# GRUB : chemin amovible, jamais de NVRAM
cat > "$MNT/tmp/debconf.txt" <<'EOF'
grub-efi-amd64 grub2/force_efi_extra_removable boolean true
grub-efi-amd64 grub2/update_nvram boolean false
keyboard-configuration keyboard-configuration/layoutcode string be
EOF
chroot "$MNT" debconf-set-selections /tmp/debconf.txt
rm "$MNT/tmp/debconf.txt"
# fbx64.efi dans EFI/BOOT tenterait d'écrire une entrée NVRAM sur le PC hôte puis redémarrerait
cat > "$MNT/etc/apt/apt.conf.d/99-no-shim-fallback" <<'EOF'
// Clé nomade : jamais de fallback shim dans le chemin amovible (écriture NVRAM + reboot)
DPkg::Post-Invoke { "rm -f /boot/efi/EFI/BOOT/fbx64.efi /boot/efi/EFI/BOOT/BOOTX64.CSV"; };
EOF

# ---------------------------------------------------------------------------
journal "Paquets : noyau, chiffrement, GRUB, Plasma minimal, firmwares"
chroot "$MNT" locale-gen
chroot "$MNT" apt-get update -qq
chroot "$MNT" env DEBIAN_FRONTEND=noninteractive apt-get install -y -o Dpkg::Options::=--force-confold \
  linux-image-amd64 cryptsetup cryptsetup-initramfs lvm2 \
  grub-efi-amd64 grub-efi-amd64-signed shim-signed \
  sudo zram-tools cloud-guest-utils gdisk whiptail \
  intel-microcode amd64-microcode \
  kde-plasma-desktop sddm plasma-nm network-manager \
  pipewire-audio wireplumber bluez bluedevil \
  task-french firefox-esr firefox-esr-l10n-fr \
  firmware-linux firmware-misc-nonfree \
  firmware-intel-graphics firmware-intel-misc firmware-mediatek firmware-nvidia-graphics \
  firmware-amd-graphics firmware-iwlwifi firmware-realtek firmware-atheros \
  firmware-brcm80211 firmware-sof-signed
for p in kde-standard kde-full task-kde-desktop; do
  if chroot "$MNT" dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok'; then
    echo "ERREUR : $p installé"; exit 1
  fi
done

# ---------------------------------------------------------------------------
journal "Service de premier démarrage"
cp -r "$ICI/fichiers/." "$MNT/"
chmod 755 "$MNT/usr/local/sbin/fabrique-cle-premier-demarrage"
mkdir -p "$MNT/var/lib/fabrique-cle"
touch "$MNT/var/lib/fabrique-cle/a-configurer"
mkdir -p "$MNT/boot/efi/fabrique-cle"
cp "$ICI/config-exemple.txt" "$MNT/boot/efi/fabrique-cle/LISEZMOI.txt"

journal "Services, zram, TRIM"
sed -i -E 's/^#?ALGO=.*/ALGO=zstd/; s/^#?PERCENT=.*/PERCENT=50/' "$MNT/etc/default/zramswap"
chroot "$MNT" systemctl enable fabrique-cle-premier-demarrage.service zramswap.service \
  fstrim.timer sddm.service NetworkManager.service
chroot "$MNT" systemctl set-default graphical.target

# ---------------------------------------------------------------------------
journal "Clavier console, initramfs, GRUB"
chroot "$MNT" setupcon --save-only
chroot "$MNT" update-initramfs -u -k all
chroot "$MNT" grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=debian \
  --uefi-secure-boot --no-nvram --force-extra-removable
rm -f "$MNT/boot/efi/EFI/BOOT/fbx64.efi" "$MNT/boot/efi/EFI/BOOT/BOOTX64.CSV"
chroot "$MNT" update-grub

# ---------------------------------------------------------------------------
journal "Nettoyage (identité unique par clé)"
chroot "$MNT" apt-get clean
rm -f "$MNT/usr/sbin/policy-rc.d"
: > "$MNT/etc/resolv.conf"
: > "$MNT/etc/machine-id"
rm -f "$MNT/var/lib/dbus/machine-id" "$MNT/var/lib/systemd/random-seed"
rm -rf "$MNT/tmp/"* "$MNT/var/log/"*.log

journal "Contrôles"
K=$(cd "$MNT" && ls boot/initrd.img-* | sort -V | tail -1)
chroot "$MNT" lsinitramfs "/$K" | grep -E 'cryptroot/(crypttab|keyfiles)|boottime.kmap' | sort
ls -l "$MNT/boot/efi/EFI/BOOT" "$MNT/boot/efi/EFI/debian"
cmp "$MNT/boot/efi/EFI/BOOT/BOOTX64.EFI" "$MNT/boot/efi/EFI/debian/shimx64.efi" && echo "BOOTX64.EFI = shim"
cmp "$MNT/boot/efi/EFI/BOOT/grubx64.efi" "$MNT/boot/efi/EFI/debian/grubx64.efi" && echo "grubx64.efi identique à EFI/debian"
[ ! -e "$MNT/boot/efi/EFI/BOOT/fbx64.efi" ] && echo "pas de fbx64.efi dans EFI/BOOT"
df -h "$MNT" "$MNT/boot" "$MNT/boot/efi"

# Blocs libres rendus au fichier image (image creuse, meilleure compression)
fstrim -v "$MNT/boot/efi" || true
fstrim -v "$MNT/boot" || true
fstrim -v "$MNT" || true

journal "Démontage"
demonter
cryptsetup status "$CRYPT" >/dev/null 2>&1 && { echo "ERREUR : volume encore ouvert"; exit 1; }

journal "Image prête : $SORTIE (clé temporaire de test : $CLE_TMP)"
