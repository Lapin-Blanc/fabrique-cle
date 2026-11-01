#!/bin/bash
# Post-installation Debian 13 « Trixie » nomade (clé SSK) — exécuté en root sur la cible.
# Idempotent : peut être relancé sans effet de bord.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
log() { printf '\n=== %s ===\n' "$*"; }

# ---------------------------------------------------------------------------
log "1. Sources APT (+ trixie-backports, non prioritaire)"
COMPONENTS="main contrib non-free non-free-firmware"
if [ -f /etc/apt/sources.list.d/debian.sources ]; then
  # Format deb822 produit par l'installeur : on le garde, on force les composants
  sed -i -E "s/^Components:.*/Components: $COMPONENTS/" /etc/apt/sources.list.d/debian.sources
  cat > /etc/apt/sources.list.d/backports.sources <<EOF
Types: deb
URIs: http://deb.debian.org/debian
Suites: trixie-backports
Components: $COMPONENTS
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg
EOF
else
  # Format une-ligne produit par l'installeur : on le garde
  sed -i -E "/^deb(-src)? /{/cdrom:/d; s/^(deb(-src)? +[^ ]+ +[^ ]+) .*/\1 $COMPONENTS/}" /etc/apt/sources.list
  echo "deb http://deb.debian.org/debian trixie-backports $COMPONENTS" > /etc/apt/sources.list.d/backports.list
fi
apt-get update -qq
# Les backports (NotAutomatic) restent à la priorité 100 : jamais choisis sans -t trixie-backports
apt-cache policy | grep -A1 'trixie-backports/main' | head -2

# ---------------------------------------------------------------------------
log "2. Plasma minimal + réseau, son, Bluetooth, français"
apt-get install -y kde-plasma-desktop sddm plasma-nm network-manager \
  pipewire-audio wireplumber bluez bluedevil \
  task-french firefox-esr firefox-esr-l10n-fr
# Garde-fou : pas de méta-paquets KDE lourds
for p in kde-standard kde-full task-kde-desktop; do
  dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok' && { echo "ERREUR : $p installé"; exit 1; }
done
# L'installeur a configuré l'interface virtuelle dans /etc/network/interfaces :
# sur un vrai PC elle n'existe pas, et NetworkManager doit gérer toutes les interfaces.
if grep -qE '^(allow-hotplug|auto) +[a-z]' /etc/network/interfaces && ! grep -q '# géré par NetworkManager' /etc/network/interfaces; then
  cp -n /etc/network/interfaces /etc/network/interfaces.installeur
  cat > /etc/network/interfaces <<'EOF'
# géré par NetworkManager (configuration de l'installeur : interfaces.installeur)
source /etc/network/interfaces.d/*
auto lo
iface lo inet loopback
EOF
fi
# Clavier belge pour SDDM et les sessions Plasma
localectl set-x11-keymap be pc105 2>/dev/null || true
systemctl set-default graphical.target

# ---------------------------------------------------------------------------
log "3. Firmwares (le matériel réel est inconnu à l'installation)"
apt-get install -y firmware-linux firmware-misc-nonfree \
  firmware-intel-graphics firmware-intel-misc firmware-mediatek firmware-nvidia-graphics \
  firmware-amd-graphics firmware-iwlwifi firmware-realtek firmware-atheros \
  firmware-brcm80211 firmware-sof-signed

# ---------------------------------------------------------------------------
log "4. Pilotes graphiques libres uniquement, initramfs MODULES=most"
if dpkg -l 'nvidia-driver*' 'nvidia-kernel-*' 2>/dev/null | grep -q '^ii'; then echo "ERREUR : pilote NVIDIA propriétaire présent"; exit 1; fi
sed -i -E 's/^MODULES=.*/MODULES=most/' /etc/initramfs-tools/initramfs.conf
# Disposition belge dans l'initramfs pour la saisie de la phrase LUKS
sed -i -E 's/^#?KEYMAP=.*/KEYMAP=y/' /etc/initramfs-tools/initramfs.conf
grep -q '^KEYMAP=y' /etc/initramfs-tools/initramfs.conf || echo 'KEYMAP=y' >> /etc/initramfs-tools/initramfs.conf
grep -E '^(MODULES|KEYMAP)=' /etc/initramfs-tools/initramfs.conf
grep -E '^XKBLAYOUT' /etc/default/keyboard

# ---------------------------------------------------------------------------
log "5. GRUB amovible persistant, sans NVRAM ni fallback shim"
debconf-set-selections <<'EOF'
grub-efi-amd64 grub2/force_efi_extra_removable boolean true
grub-efi-amd64 grub2/update_nvram boolean false
EOF
# fbx64.efi dans EFI/BOOT tenterait d'écrire une entrée NVRAM sur le PC hôte puis redémarrerait
cat > /etc/apt/apt.conf.d/99-no-shim-fallback <<'EOF'
// Clé nomade : jamais de fallback shim dans le chemin amovible (écriture NVRAM + reboot)
DPkg::Post-Invoke { "rm -f /boot/efi/EFI/BOOT/fbx64.efi /boot/efi/EFI/BOOT/BOOTX64.CSV"; };
EOF
rm -f /boot/efi/EFI/BOOT/fbx64.efi /boot/efi/EFI/BOOT/BOOTX64.CSV

# ---------------------------------------------------------------------------
log "6. Horloge matérielle en heure locale (cohabitation Windows)"
timedatectl set-local-rtc 1 --adjust-system-clock

# ---------------------------------------------------------------------------
log "7. zram (zstd), pas d'hibernation"
apt-get install -y zram-tools
sed -i -E 's/^#?ALGO=.*/ALGO=zstd/; s/^#?PERCENT=.*/PERCENT=50/' /etc/default/zramswap
systemctl enable zramswap.service
echo 'RESUME=none' > /etc/initramfs-tools/conf.d/resume

# ---------------------------------------------------------------------------
log "8. TRIM de bout en bout"
# discard sur le volume LUKS (4e champ de crypttab)
awk 'BEGIN{OFS=" "} /^[^#]/ && NF>=4 && $4 !~ /(^|,)discard(,|$)/ {$4=$4",discard"} {print}' /etc/crypttab > /etc/crypttab.new
mv /etc/crypttab.new /etc/crypttab
cat /etc/crypttab
systemctl enable fstrim.timer

# ---------------------------------------------------------------------------
log "Regénération initramfs + GRUB"
update-initramfs -u -k all
update-grub

log "Contrôles"
K=$(ls /boot/initrd.img-* | sort -V | tail -1)
lsinitramfs "$K" | grep -E 'boottime.kmap|cryptroot/crypttab' || true
unmkinitramfs "$K" /tmp/ir >/dev/null 2>&1 && { cat /tmp/ir/main/cryptroot/crypttab 2>/dev/null || cat /tmp/ir/cryptroot/crypttab; rm -rf /tmp/ir; }
ls -l /boot/efi/EFI/BOOT /boot/efi/EFI/debian
cmp /boot/efi/EFI/BOOT/BOOTX64.EFI /boot/efi/EFI/debian/shimx64.efi && echo "BOOTX64.EFI = shim"
cmp /boot/efi/EFI/BOOT/grubx64.efi /boot/efi/EFI/debian/grubx64.efi && echo "grubx64.efi identique à EFI/debian"
cat /boot/efi/EFI/BOOT/grub.cfg 2>/dev/null || true
cat /boot/efi/EFI/debian/grub.cfg
grep -vE '^#|^$' /etc/fstab
grep -E '^GRUB_CMDLINE' /etc/default/grub
debconf-show grub-efi-amd64 | grep -E 'removable|nvram'
timedatectl | grep -iE 'RTC in local|local TZ'
systemctl is-enabled zramswap fstrim.timer sddm NetworkManager bluetooth
ls /usr/share/locale/fr/LC_MESSAGES/ | grep -cE '^(plasma|kcm|kio|dolphin|konsole)' | sed 's/^/catalogues KDE fr : /'
echo "POSTINSTALL OK"
