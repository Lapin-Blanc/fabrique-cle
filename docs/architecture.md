# Architecture

```
GitHub Actions (ubuntu-24.04, root, KVM)                 PC de l'élève (Windows/Linux)           PC quelconque
┌───────────────────────────────┐   release v*   ┌──────────────────────────────┐  écrit  ┌──────────────────────┐
│ construire-image.sh (mmdebstrap)│ ─────────────▶ │ app Rust : télécharge, vérifie│ ──────▶ │ clé USB : 1er démarrage│
│ tester-image.sh (QEMU+OVMF SB)  │  .img.zst      │ écrit, dépose config.txt (ESP)│         │ applique config.txt   │
└───────────────────────────────┘  .bmap .sha256 └──────────────────────────────┘         └──────────────────────┘
```

## 1. Image (`image/construire-image.sh`)

Fichier image de 10 Gio (creux), monté en loop, dans un **espace de montage privé** (`unshare`).

| Partition | Taille | Contenu |
|---|---|---|
| p1 ESP | 512 Mio, FAT32, étiquette `FABCLE` | shim signé + GRUB en chemin amovible `EFI/BOOT/BOOTX64.EFI` ; dossier `fabrique-cle/` (config) |
| p2 /boot | 1 Gio, ext4 | noyaux, initramfs |
| p3 | reste, type 8309 | LUKS2 (`fabcle_crypt`) → LVM `fabcle/root` ext4 |

- Système : `mmdebstrap --variant=important` trixie (main contrib non-free non-free-firmware), puis en chroot :
  noyau, cryptsetup-initramfs, lvm2, grub-efi-amd64-signed + shim-signed, `kde-plasma-desktop sddm plasma-nm
  network-manager pipewire-audio wireplumber bluez bluedevil task-french firefox-esr(+fr)`, microcodes,
  firmwares (liste dans le script), zram-tools, cloud-guest-utils, whiptail. Pas de serveur SSH.
- Sources APT au format une-ligne + `trixie-backports` (priorité 100 par défaut).
- initramfs : `MODULES=most`, `KEYMAP=y`, `RESUME=none` (`/etc/initramfs-tools/conf.d/fabrique-cle.conf`).
- crypttab : `fabcle_crypt UUID=… /etc/cryptsetup-keys.d/fabcle_crypt.key luks,discard,initramfs` —
  la **clé temporaire** de l'image est incluse dans l'initramfs (`KEYFILE_PATTERN`) : le 1er démarrage ne demande rien.
- GRUB : `grub-install --uefi-secure-boot --no-nvram --force-extra-removable`, debconf
  `force_efi_extra_removable=true`, `update_nvram=false` ; hook APT qui supprime `EFI/BOOT/fbx64.efi`.
- RTC en heure locale (`/etc/adjtime` LOCAL), fuseau Europe/Brussels, zram zstd 50 %, `fstrim.timer`.
- `machine-id` vide (régénéré par clé), pas de compte utilisateur (créé au 1er démarrage), root verrouillé.
- Fin : `fstrim` (image creuse → bonne compression), démontage strict.

## 2. Premier démarrage (`fabrique-cle-premier-demarrage` + service systemd)

Service `oneshot` sur tty1, `Before=display-manager.service`, condition `/var/lib/fabrique-cle/a-configurer`.
Étapes (chacune marquée `fait-*` dans `/var/lib/fabrique-cle`, donc rejouable) :

1. Lecture de `/boot/efi/fabrique-cle/config.txt` (clé=valeur, jamais exécuté, CRLF toléré) ; valeurs
   manquantes ou invalides demandées avec whiptail.
2. Clavier appliqué immédiatement (setupcon) pour la saisie des secrets ; langue (locale-gen), nom de machine.
3. Compte : `useradd`, groupes (sudo, audio, video, plugdev, netdev, bluetooth…), hachage fourni ou saisi.
4. Si chiffrement : `cryptsetup reencrypt` **en ligne** (nouvelle clé maîtresse ; ~1 min pour l'image) —
   avant l'agrandissement pour ne rechiffrer que la taille de l'image.
5. Agrandissement : `sgdisk -e`, `growpart`, `cryptsetup resize`, `pvresize`, `lvextend -r`.
6. Si chiffrement : ajout de la phrase de l'élève, retrait de la clé temporaire (slot + fichier + crypttab).
   Sans chiffrement : la clé temporaire reste (déverrouillage automatique).
7. `update-initramfs -u -k all`, effacement de `config.txt`, puis SDDM démarre (pas de redémarrage).

Clé de test `eteindre_apres=oui` et `phrase_luks_test=` : réservées au test automatique.

## 3. Contrat application ↔ image : `config.txt`

Voir `image/config-exemple.txt`. Clés : `identifiant`, `nom_complet`, `mot_de_passe_hache` (`$6$…`),
`machine`, `clavier`, `langue`, `chiffrer`. Toute évolution doit rester rétrocompatible (une ancienne
application avec une nouvelle image et inversement).

## 4. Application (`app/`)

Rust, interface `egui`/`eframe` 0.37 (un binaire, Windows + Linux), une fenêtre style Rufus :
clé USB → image (version publiée ou fichier local glissé) → personnalisation → Démarrer → confirmation.

| Module | Rôle |
|---|---|
| `config.rs` | champs, validation **identique** au premier démarrage (regex, listes), `config.txt`, hachage SHA-512 crypt (sel 16 caractères, cf. `lecons.md`) |
| `releases.rs` | versions via l'API GitHub ; si quota dépassé (60 req/h par IP, NAT d'école) : `releases/latest/download/…` ; téléchargement des morceaux `*.part00…` dans un cache indexé par SHA-256 (réutilisé pour plusieurs clés) ; certificats du système (proxys d'école) |
| `ecriture.rs` | zstd en flux, n'écrit que les plages du `.bmap`, efface le dernier Mio (ancienne GPT de secours), puis **relit et vérifie** chaque plage (SHA-256 du bmap, cache noyau vidé) |
| `esp.rs` | lit la GPT, trouve l'ESP, écrit `fabrique-cle/config.txt` avec `fatfs` à travers une fenêtre aux E/S alignées sur 4096 (exigé par Windows), puis relit |
| `disques/linux.rs` | sysfs : disques dont le chemin passe par `/usb`, hors disques système (racine, /boot, /home…, swap, en traversant LUKS/LVM via `slaves`) ; refus si un volume LUKS/LVM est ouvert ; `umount`, ouverture `O_EXCL`, `BLKFLSBUF`, `BLKRRPART` |
| `disques/windows.rs` | `\\.\PhysicalDriveN` en bus USB (ou amovible), hors disque de `C:\Windows` ; verrouillage + démontage de chaque volume de la clé (`FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`), `IOCTL_DISK_UPDATE_PROPERTIES` à la fin ; manifeste `requireAdministrator` (`build.rs`) |
| `assistant.rs` | Linux : l'interface reste utilisateur ; l'écriture est faite par `pkexec <app> --assistant` (JSON sur stdin, progression ligne à ligne sur stdout, fermeture de stdin = annulation). Dans une AppImage, c'est `$APPIMAGE` qui est relancé (root ne lit pas le montage FUSE) |
| `travail.rs` | enchaînement : ouverture → écriture → vérification → `config.txt` → relecture |

Garde-fous : la clé choisie est **revérifiée** (chemin + n° de série + taille) juste avant l'écriture,
y compris par l'assistant root ; la confirmation affiche modèle, taille et n° de série.

Tests : `cd app && cargo test` (parcours complet sur fichiers : GPT + FAT32 compressée, bmap,
écriture, vérification, config.txt). Essai avec une image réelle écrite dans un fichier :
`FABCLE_IMAGE=x.img.zst FABCLE_BMAP=x.img.bmap FABCLE_CIBLE=cle.bin cargo test -- --ignored`.

## 5. CI (`.github/workflows/image.yml`, `app.yml`)

`app.yml` (push sur `app/**`, sur `main` et les branches `claude/**`) : tests + compilation Linux
(ubuntu-22.04, binaire + AppImage) et Windows (`.exe`), en artefacts 14 jours ; sur étiquette `v*`,
joints à la release de l'image (`fabrique-cle-windows.exe`, `fabrique-cle-linux-x86_64.AppImage`).


Push sur `image/**` : construction → test QEMU → compression zstd + bmap + sha256 → artefact 7 jours.
Étiquette `v*` : idem + release GitHub. Le test démarre une **copie agrandie de 6 Gio** de l'image en
**clé USB xHCI**, OVMF **Secure Boot avec clés Microsoft**, NVRAM vierge, puis inspecte la clé hors ligne
(14 vérifications). En cas d'échec : capture d'écran `ecran.ppm` et journal en artefact.
