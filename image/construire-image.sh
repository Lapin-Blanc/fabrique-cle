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
# Le volume LUKS2 de l'image est créé SANS chiffrement réel (cipher_null) : son contenu reste en
# clair, donc compressible (un volume chiffré ne se compresse pas : 7,5 Go au lieu de ~2 Go). Il est
# ouvert par une clé temporaire incluse dans l'initramfs ; si chiffrer=oui, le service de premier
# démarrage le chiffre en ligne en AES-XTS (nouvelle clé maîtresse, propre à chaque clé USB), puis
# remplace la clé temporaire par la phrase de l'utilisateur.
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
# Espace de montage privé : nos montages ne se propagent pas aux services de l'hôte
# (qui les garderaient ouverts) et disparaissent avec le script
[ -n "${FC_PRIVE:-}" ] || exec env FC_PRIVE=1 unshare --mount --propagation private "$0" "$@"
for o in mmdebstrap sgdisk cryptsetup pvcreate mkfs.vfat mkfs.ext4 partx; do
  command -v "$o" >/dev/null || { echo "Outil manquant : $o"; exit 1; }
done

journal() { printf '\n=== %s ===\n' "$*"; }

MNT=$(mktemp -d)
# Cache APT HORS de l'image : les .deb téléchargés (déjà compressés) laisseraient, une fois effacés,
# ~1,5 Gio de blocs libres incompressibles que fstrim ne rend pas tous au fichier image
mkdir -p "$(dirname "$SORTIE")"
CACHE=$(mktemp -d -p "$(dirname "$SORTIE")" cache-apt.XXXXXX)
LOOP=
# Processus lancés dans le chroot par les scripts des paquets (ils bloqueraient le démontage)
tuer_chroot() {
  local p
  for p in /proc/[0-9]*; do
    [ "$(readlink "$p/root" 2>/dev/null)" = "$MNT" ] && { echo "arrêt du processus ${p#/proc/} ($(cat "$p/comm"))"; kill -9 "${p#/proc/}"; }
  done
  sleep 1
}
# Rend au fichier image TOUS les blocs libres des systèmes ext4, une fois démontés : fstrim sur un
# système monté en oublie une part variable (2,8 à 3 Gio rendus selon les builds, soit ~90 Mo de
# plus ou de moins une fois compressé). e2fsck -E discard est exhaustif et vérifie au passage.
liberer_blocs() {
  local d rc
  for d in "$P2" "/dev/$VG/root"; do
    rc=0; e2fsck -f -p -E discard "$d" || rc=$?
    [ "$rc" -le 1 ] || { echo "ERREUR : e2fsck $d (code $rc)"; exit 1; }
  done
}
demonter() {
  tuer_chroot
  umount -R "$MNT"
  liberer_blocs
  # udev sonde les périphériques juste après le démontage : on attend qu'il les relâche
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    udevadm settle || true
    vgchange -an "$VG" >/dev/null 2>&1 && break
    [ "$i" -eq 10 ] && { grep -l "$VG" /proc/[0-9]*/mountinfo; fuser -vm "/dev/$VG/root"; dmsetup info -c; vgchange -an "$VG"; }
    sleep 3
  done
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
  rm -rf "$CACHE"
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

journal "LUKS2 cipher_null (clé temporaire) -> LVM -> systèmes de fichiers"
CLE_TMP="${SORTIE%.img}.key"
head -c 64 /dev/urandom > "$CLE_TMP"
chmod 600 "$CLE_TMP"
# cipher_null : données écrites en clair (compressibles) ; l'emplacement de clé, lui, reçoit le
# chiffrement par défaut de cryptsetup
cryptsetup luksFormat --batch-mode --type luks2 --label fabcle --cipher cipher_null-ecb \
  --key-file "$CLE_TMP" "$P3"
ENTETE=$(cryptsetup luksDump "$P3")
grep -E '^\s+cipher:' <<<"$ENTETE"
grep -qE '^\s+cipher:\s+cipher_null' <<<"$ENTETE" || { echo "ERREUR : volume non cipher_null"; exit 1; }
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
# Allègement : ni documentation ni pages de manuel ni info (≈ 150-200 Mo). mmdebstrap écrit ces règles
# dans /etc/dpkg/dpkg.cfg.d/99mmdebstrap : elles valent aussi pour les paquets installés ensuite, et
# sur la clé. Les licences (copyright) restent. Firmwares et pilotes : tous gardés (clé polyvalente).
# Pour récupérer les pages de manuel sur une clé : supprimer ce fichier et réinstaller les paquets.
ALLEGER=(
  --dpkgopt='path-exclude=/usr/share/doc/*'
  --dpkgopt='path-include=/usr/share/doc/*/copyright'
  --dpkgopt='path-exclude=/usr/share/man/*'
  --dpkgopt='path-exclude=/usr/share/info/*'
  --dpkgopt='path-exclude=/usr/share/lintian/*'
)
mmdebstrap --variant=important --components="$COMPOSANTS" \
  --setup-hook="mkdir -p \"\$1/var/cache/apt/archives\" && mount --bind '$CACHE' \"\$1/var/cache/apt/archives\"" \
  --customize-hook='umount "$1/var/cache/apt/archives"' \
  --include=locales,console-setup,keyboard-configuration,ca-certificates \
  "${ALLEGER[@]}" \
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
/dev/mapper/$VG-root                    /                  ext4   errors=remount-ro,noatime,commit=30 0 1
UUID=$UUID_BOOT /boot              ext4   defaults,noatime          0      2
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
# L'initramfs de l'image doit savoir ouvrir le volume cipher_null (inoffensif après chiffrement)
grep -qx crypto_null "$MNT/etc/initramfs-tools/modules" 2>/dev/null || echo crypto_null >> "$MNT/etc/initramfs-tools/modules"

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
mkdir -p "$CACHE/partial"
mount --bind "$CACHE" "$MNT/var/cache/apt/archives"
# Nos fichiers d'abord : dépôt Brave (et sa clé) nécessaires à l'installation, réglages pris en
# compte par les paquets. Recopiés après l'installation, pour qu'aucun paquet ne les remplace.
cp -r "$ICI/fichiers/." "$MNT/"
chroot "$MNT" apt-get update -qq
chroot "$MNT" env DEBIAN_FRONTEND=noninteractive apt-get install -y -o Dpkg::Options::=--force-confold \
  linux-image-amd64 cryptsetup cryptsetup-initramfs lvm2 \
  grub-efi-amd64 grub-efi-amd64-signed shim-signed \
  sudo pkexec zram-tools cloud-guest-utils gdisk whiptail systemd-timesyncd \
  intel-microcode amd64-microcode \
  kde-plasma-desktop sddm plasma-nm network-manager \
  pipewire-audio wireplumber bluez bluedevil \
  task-french brave-browser curl plasma-wallpapers-addons konqueror- firefox-esr- chromium- \
  firmware-linux firmware-misc-nonfree \
  firmware-intel-graphics firmware-intel-misc firmware-mediatek firmware-nvidia-graphics \
  firmware-amd-graphics firmware-iwlwifi firmware-realtek firmware-atheros \
  firmware-brcm80211 firmware-sof-signed \
  fonts-noto-extra- fonts-noto-cjk-extra- fonts-noto-ui-extra-
# (« paquet- » = ne pas installer.) Navigateurs : Brave seul ; webext-plasma-browser-integration
# (recommandé par Plasma) recommande « firefox-esr | firefox | chromium » : les exclure tous deux.
# Polices : ces trois paquets, recommandés via fonts-noto, n'apportent que des
# graisses et largeurs supplémentaires (~600 Mio) : toutes les écritures restent couvertes par
# fonts-noto-core, fonts-noto-cjk, fonts-noto-ui-core et fonts-noto-mono.
for p in kde-standard kde-full task-kde-desktop fonts-noto-extra fonts-noto-cjk-extra fonts-noto-ui-extra \
         konqueror firefox-esr chromium; do
  if chroot "$MNT" dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok'; then
    echo "ERREUR : $p installé"; exit 1
  fi
done
# pkexec : l'application Fabrique-Clé, lancée depuis la clé, en a besoin pour obtenir les droits
# administrateur (fabriquer une autre clé, mesures du diagnostic). Paquet séparé de polkitd.
chroot "$MNT" test -x /usr/bin/pkexec || { echo "ERREUR : pkexec absent"; exit 1; }

# ---------------------------------------------------------------------------
journal "Service de premier démarrage"
cp -r "$ICI/fichiers/." "$MNT/"
chmod 755 "$MNT/usr/local/sbin/fabrique-cle-premier-demarrage"
mkdir -p "$MNT/var/lib/fabrique-cle"
touch "$MNT/var/lib/fabrique-cle/a-configurer"
mkdir -p "$MNT/boot/efi/fabrique-cle"
cp "$ICI/config-exemple.txt" "$MNT/boot/efi/fabrique-cle/LISEZMOI.txt"

# Brave, navigateur par défaut de Plasma (sinon Plasma choisit lui-même, souvent Konqueror)
if chroot "$MNT" sh -c 'command -v kwriteconfig6' >/dev/null; then
  chroot "$MNT" kwriteconfig6 --file /etc/xdg/kdeglobals --group General --key BrowserApplication brave-browser.desktop
else
  printf '[General]\nBrowserApplication=brave-browser.desktop\n' >> "$MNT/etc/xdg/kdeglobals"
fi

journal "Services, zram, TRIM"
sed -i -E 's/^#?ALGO=.*/ALGO=zstd/; s/^#?PERCENT=.*/PERCENT=50/' "$MNT/etc/default/zramswap"
chroot "$MNT" systemctl enable fabrique-cle-premier-demarrage.service zramswap.service \
  fstrim.timer sddm.service NetworkManager.service
chroot "$MNT" systemctl set-default graphical.target
# Inutiles ou coûteux sur une clé : attente du réseau au démarrage, modem 4G, SMART (rarement
# disponible en USB), rafraîchissement des firmwares. Aucune mise à jour automatique : Discover
# signale les mises à jour, l'utilisateur les lance quand cela l'arrange.
for u in NetworkManager-wait-online.service ModemManager.service smartmontools.service fwupd-refresh.timer; do
  chroot "$MNT" systemctl disable "$u" 2>/dev/null || true
done
chroot "$MNT" systemctl mask apt-daily.timer apt-daily-upgrade.timer

# ---------------------------------------------------------------------------
journal "Clavier console, initramfs, GRUB"
chroot "$MNT" setupcon --save-only
chroot "$MNT" update-initramfs -u -k all
chroot "$MNT" grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=debian \
  --uefi-secure-boot --no-nvram --force-extra-removable
rm -f "$MNT/boot/efi/EFI/BOOT/fbx64.efi" "$MNT/boot/efi/EFI/BOOT/BOOTX64.CSV"
sed -i -E 's/^GRUB_TIMEOUT=.*/GRUB_TIMEOUT=1/' "$MNT/etc/default/grub"   # menu : Échap pour l'afficher
chroot "$MNT" update-grub

# ---------------------------------------------------------------------------
journal "Nettoyage (identité unique par clé)"
chroot "$MNT" apt-get clean
# Restes de l'allègement : le jeu essentiel de mmdebstrap est extrait sans les règles path-exclude
find "$MNT/usr/share/doc" -mindepth 1 ! -type d ! -name copyright -delete
find "$MNT/usr/share/doc" -mindepth 1 -type d -empty -delete
rm -rf "$MNT/usr/share/man/"* "$MNT/usr/share/info/"* "$MNT/usr/share/lintian/"*
umount "$MNT/var/cache/apt/archives"
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
cryptsetup status "$CRYPT" | grep -E 'cipher|keysize'
du -sm "$MNT/usr/share/doc" "$MNT/usr/share/man" "$MNT/usr/lib/firmware" 2>/dev/null || true

# Blocs libres rendus au fichier image (image creuse, meilleure compression)
# (ext4 : e2fsck -E discard au démontage ; la FAT, elle, ne se traite que montée)
fstrim -v "$MNT/boot/efi" || true
sync

journal "Démontage"
demonter
cryptsetup status "$CRYPT" >/dev/null 2>&1 && { echo "ERREUR : volume encore ouvert"; exit 1; }

echo "Blocs alloués dans le fichier image : $(du -m "$SORTIE" | cut -f1) Mio"
journal "Image prête : $SORTIE (clé temporaire de test : $CLE_TMP)"
