# Notes de conception

Reprises de l'installation manuelle validée (clé SSK, 2026-10-30) :

- GPT : ESP 512 Mio (FAT32), /boot 1 Gio (ext4), LUKS2 -> LVM vg/root (ext4), pas de swap
- GRUB en chemin amovible `EFI/BOOT/BOOTX64.EFI` (= shim signé) + `grubx64.efi` identique à `EFI/debian` ;
  jamais `fbx64.efi` dans `EFI/BOOT` (écriture NVRAM + reboot) ; `update_nvram=false`
- initramfs : `MODULES=most`, `KEYMAP=y` (clavier de la phrase LUKS), `RESUME=none`
- crypttab `discard`, `fstrim.timer` ; zram zstd 50 %
- RTC en heure locale (cohabitation Windows)
- Plasma minimal : `kde-plasma-desktop sddm plasma-nm network-manager pipewire-audio wireplumber bluez bluedevil`
  + firmwares (liste dans `image/scripts/postinstall-reference.sh`)
- `/etc/network/interfaces` réduit à `lo` (NetworkManager gère tout)

Nouveau pour la diffusion :

- l'image publiée est publique -> sa clé maîtresse LUKS l'est aussi : `cryptsetup reencrypt` obligatoire au 1er démarrage
- premier démarrage : lecture de `EFI/fabrique-cle/config` (FAT, écrit par l'application), puis effacement
- mot de passe transmis haché (yescrypt / SHA-512), jamais en clair
- release GitHub : 2 Gio max par fichier -> image découpée si nécessaire
