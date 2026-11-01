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
| p3 | reste, type 8309 | LUKS2 (`fabcle_crypt`, **`cipher_null`** dans l'image) → LVM `fabcle/root` ext4 |

- Système : `mmdebstrap --variant=important` trixie (main contrib non-free non-free-firmware), puis en chroot :
  noyau, cryptsetup-initramfs, lvm2, grub-efi-amd64-signed + shim-signed, `kde-plasma-desktop sddm plasma-nm
  network-manager pipewire-audio wireplumber bluez bluedevil task-french firefox-esr(+fr)`, microcodes,
  firmwares (liste dans le script), zram-tools, cloud-guest-utils, whiptail. Pas de serveur SSH.
- Sources APT au format une-ligne + `trixie-backports` (priorité 100 par défaut).
- initramfs : `MODULES=most`, `KEYMAP=y`, `RESUME=none` (`/etc/initramfs-tools/conf.d/fabrique-cle.conf`),
  module `crypto_null` (pour ouvrir le volume de l'image).
- **Volume de l'image en `cipher_null`** : LUKS2 « vide » (données en clair, emplacement de clé chiffré),
  pour que l'image publiée reste compressible (~2 Go au lieu de 7,5 Go avec un vrai chiffrement). Le
  vrai chiffrement est fait sur chaque clé au premier démarrage (étape 4).
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
4. Si chiffrement : `cryptsetup reencrypt --cipher aes-xts-plain64 --key-size 512` **en ligne**
   (`cipher_null` → AES-XTS, nouvelle clé maîtresse ; ~1 min pour l'image) — avant l'agrandissement pour
   ne traiter que la taille de l'image. Reprise après coupure (`--resume-only` si l'en-tête porte
   `online-reencrypt`), contrôle final : plus aucun segment `cipher_null`.
5. Agrandissement : `sgdisk -e`, `growpart`, `cryptsetup resize`, `pvresize`, `lvextend -r`.
6. Si chiffrement : ajout de la phrase de l'élève, retrait de la clé temporaire (slot + fichier + crypttab).
   Sans chiffrement : le volume reste en `cipher_null` (réellement non chiffré) et la clé temporaire
   reste (déverrouillage automatique).
7. `update-initramfs -u -k all`, effacement de `config.txt`, puis SDDM démarre (pas de redémarrage).

Clé de test `eteindre_apres=oui` et `phrase_luks_test=` : réservées au test automatique.

## 3. Contrat application ↔ image : `config.txt`

Voir `image/config-exemple.txt`. Clés : `identifiant`, `nom_complet`, `mot_de_passe_hache` (`$6$…`),
`machine`, `clavier`, `langue`, `chiffrer`. Toute évolution doit rester rétrocompatible (une ancienne
application avec une nouvelle image et inversement).

## 4. Application (`app/`, à faire)

- Rust, interface `egui`/`eframe` (un binaire, Windows + Linux). Style Rufus : une fenêtre, de haut en bas.
- Liste des disques : USB/amovibles uniquement, jamais le disque système ; confirmation avant effacement.
- Écriture brute : Windows → verrouiller/démonter les volumes (`FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`),
  écrire `\\.\PhysicalDriveN`, manifeste « requireAdministrator » ; Linux → udisks2 (`OpenDevice`, polkit)
  ou relance via `pkexec`.
- Décompression zstd en flux, saut des blocs vides grâce au `.bmap`, vérification SHA-256.
- Écriture de `config.txt` **dans la FAT de la partition 1 directement sur le périphérique brut** (crate
  `fatfs`), car Windows n'attribue pas de lettre à une ESP.
- Mot de passe haché localement (crate `sha-crypt`, SHA-512 crypt).
- Liste des versions : API GitHub releases du dépôt ; image en morceaux `*.part00…` si > 2 Gio.

## 5. CI (`.github/workflows/image.yml`)

Push sur `image/**` : construction → test QEMU → compression zstd + bmap + sha256 → artefact 7 jours.
Garde-fou : échec si l'image compressée dépasse 4 Go (signe d'un volume chiffré dans l'image).
Étiquette `v*` : idem + release GitHub. Le test démarre une **copie agrandie de 6 Gio** de l'image en
**clé USB xHCI**, OVMF **Secure Boot avec clés Microsoft**, NVRAM vierge, puis inspecte la clé hors ligne
(15 vérifications, dont « AES-XTS, plus de `cipher_null` »). En cas d'échec : capture d'écran `ecran.ppm` et journal en artefact.
