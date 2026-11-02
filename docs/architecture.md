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
- Navigateur : **Brave** seul (dépôt officiel, trousseau versionné dans `image/fichiers/usr/share/keyrings/`,
  tenu à jour ensuite par le paquet `brave-keyring`), par défaut via `/etc/xdg/mimeapps.list` et
  `kdeglobals` ; Konqueror et Firefox exclus. `plasma-wallpapers-addons` : fond
  « image du jour » (Bing) appliqué une fois à la 1re session (`/etc/xdg/autostart`, `evaluateScript`).
- Réglages pour clé USB lente, neutres sur un SSD rapide : écriture différée bornée (16/64 Mio),
  zram favorisé (`swappiness=150`, `page-cluster=0`), BFQ + lecture anticipée 512 Ko pour les disques
  USB (udev `61-…`), racine en `noatime,commit=30`, journal 64 Mo synchronisé toutes les 30 s,
  `~/.cache` en tmpfs (ajouté au premier démarrage), GRUB à 1 s.
- Mises à jour : **aucune automatique** (`apt-daily*` masqués, `fwupd-refresh` désactivé) ; Discover
  les signale, l'utilisateur les lance quand cela l'arrange. Désactivés aussi : `NetworkManager-wait-online`,
  `ModemManager`, `smartmontools`. `fstrim.timer` reste actif (TRIM utile sur SSD USB en UAS).
- RTC en heure locale (`/etc/adjtime` LOCAL), `systemd-timesyncd` (heure réseau), fuseau Europe/Brussels, zram zstd 50 %, `fstrim.timer`.
- `machine-id` vide (régénéré par clé), pas de compte utilisateur (créé au 1er démarrage), root verrouillé.
- Cache APT **hors de l'image** (montage lié d'un dossier de l'hôte sur `/var/cache/apt/archives`, aussi
  pendant mmdebstrap) : les `.deb` effacés laisseraient des blocs libres incompressibles.
- Allègement (firmwares et pilotes tous gardés : clé polyvalente) : pas de documentation, de pages de
  manuel ni d'info (`path-exclude` dpkg écrits par mmdebstrap dans `/etc/dpkg/dpkg.cfg.d/99mmdebstrap`,
  licences gardées) ; pas de `fonts-noto-extra`, `fonts-noto-cjk-extra`, `fonts-noto-ui-extra` (graisses
  supplémentaires seulement : toutes les écritures restent couvertes).
- Fin : `fstrim` de l'ESP, puis au démontage `e2fsck -f -p -E discard` sur `/boot` et la racine (tous les
  blocs libres rendus au fichier, de façon reproductible), démontage strict.

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

## 4. Application (`app/`)

Rust, interface `egui`/`eframe` 0.37 (un binaire, Windows + Linux), une fenêtre style Rufus :
clé USB → image (version publiée ou fichier local glissé) → personnalisation → Démarrer → confirmation.

| Module | Rôle |
|---|---|
| `config.rs` | champs, validation **identique** au premier démarrage (regex, listes), `config.txt`, hachage SHA-512 crypt (sel 16 caractères, cf. `lecons.md`) |
| `releases.rs` | versions via l'API GitHub ; si quota dépassé (60 req/h par IP, NAT d'école) : `releases/latest/download/…` ; téléchargement des morceaux `*.part00…` dans un cache indexé par SHA-256 (réutilisé pour plusieurs clés) ; certificats du système (proxys d'école) |
| `telechargement.rs` | téléchargement en 8 connexions parallèles (morceaux de 8 Mio, requêtes `Range`), écrits à leur place dans le fichier ; fichier `.etat` des morceaux terminés : **reprise** après coupure ; repli sur un seul flux si les morceaux ne passent pas |
| `ecriture.rs` | zstd en flux (fenêtre jusqu'à 2 Gio : l'image est compressée en `--long=27`), n'écrit que les plages du `.bmap`, efface le dernier Mio (ancienne GPT de secours), puis **relit et vérifie** chaque plage (SHA-256 du bmap, cache noyau vidé) |
| `esp.rs` | lit la GPT, trouve l'ESP, écrit `fabrique-cle/config.txt` avec `fatfs` à travers une fenêtre aux E/S alignées sur 4096 (exigé par Windows), puis relit |
| `disques/linux.rs` | sysfs : disques dont le chemin passe par `/usb`, hors disques système (racine, /boot, /home…, swap, en traversant LUKS/LVM via `slaves`) ; refus si un volume LUKS/LVM est ouvert ; `umount`, ouverture `O_EXCL`, `BLKFLSBUF`, `BLKRRPART` |
| `disques/windows.rs` | `\\.\PhysicalDriveN` en bus USB (ou amovible), hors disque de `C:\Windows` ; verrouillage + démontage de chaque volume de la clé (`FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`), `IOCTL_DISK_UPDATE_PROPERTIES` à la fin ; manifeste `requireAdministrator` (`build.rs`) |
| `assistant.rs` | Linux : l'interface reste utilisateur ; l'écriture est faite par `pkexec <app> --assistant` (JSON sur stdin, progression ligne à ligne sur stdout, fermeture de stdin = annulation). Dans une AppImage, c'est `$APPIMAGE` qui est relancé (root ne lit pas le montage FUSE) |
| `theme.rs` | apparence : police Inter embarquée (OFL, `app/ressources/`), couleurs claires et sombres suivant le système (`FABRIQUE_CLE_THEME=clair\|sombre` pour forcer), espacements, icône de fenêtre |
| `interface.rs` | fenêtre : en-tête, trois cartes numérotées (clé, système, compte), barre du bas toujours visible (état, bouton « Fabriquer la clé ») |
| `travail.rs` | enchaînement : ouverture → écriture → vérification → `config.txt` → relecture |
| `diagnostic/` | onglet **Diagnostic**, purement informatif, indépendant de la fabrication (voir ci-dessous) |
| `interface/diagnostic.rs` | affichage de l'onglet Diagnostic (identification, mesures, histogramme, fiches) |

Garde-fous : la clé choisie est **revérifiée** (chemin + n° de série + taille) juste avant l'écriture,
y compris par l'assistant root ; la confirmation affiche modèle, taille et n° de série.

Tests : `cd app && cargo test` (parcours complet sur fichiers : GPT + FAT32 compressée, bmap,
écriture, vérification, config.txt). Essai avec une image réelle écrite dans un fichier :
`FABCLE_IMAGE=x.img.zst FABCLE_BMAP=x.img.bmap FABCLE_CIBLE=cle.bin cargo test -- --ignored`.

### Onglet Diagnostic

Outil didactique : examine une clé sous l'angle de notre usage (un système qui lit et écrit sans cesse
de petits blocs), sans rien changer au parcours de fabrication. Les mesures en lecture n'écrivent
rien ; seule l'**analyse complète** écrit sur la clé, après confirmation (modèle, taille, n° de série).

| Module | Rôle |
|---|---|
| `diagnostic/materiel.rs` | `Materiel` : VID:PID, pilote (uas / usb-storage, UASPStor / USBSTOR), norme USB, liaison négociée, TRIM, file de commandes… Table `ressources/diagnostic/fabricants-usb.txt` (marque, contrôleur, pont USB-SSD, suspect) |
| `disques/linux.rs` `identifier` | sysfs, sans droits : `queue/discard_max_bytes`, `device/queue_depth`, puis remontée jusqu'à l'interface USB (`driver`) et au périphérique (`idVendor`, `version`, `speed`) |
| `disques/windows.rs` `identifier` | `IOCTL_STORAGE_QUERY_PROPERTY` (TRIM, alignement, adaptateur) ; SetupAPI + CfgMgr : nœud du disque → nœud `USB\VID_…` (service = pilote) → concentrateur : `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX(_V2)` (vitesse, bcdUSB, SuperSpeed possible) |
| `diagnostic/mesures.rs` | lectures **directes** (`O_DIRECT` / `FILE_FLAG_NO_BUFFERING`, tampons alignés 4096), bornées en durée : séquentielle 4 Mio (8 s), aléatoire 4 Kio une à la fois (8 s), puis 8 à la fois (8 s, un descripteur par fil) ; latences (médiane, 90/99 %, max, histogramme) ; part de blocs vides (lectures optimistes) |
| `diagnostic/analyse.rs` | analyse complète (**efface la clé**), ouverte comme pour la fabrication (`disques::ouvrir` : revérification, démontage / verrouillage) puis 8 descripteurs directs (`ouvrir_ecriture`) : 1) capacité réelle : marqueurs sur une grille au pas puissance de deux (un repliement d'adresses de contrefaçon écrase un marqueur déjà écrit), relus ; 2) écriture aléatoire 4 Kio, 1 puis 8 à la fois ; 3) écritures synchronisées (fdatasync / FlushFileBuffers à chaque bloc) ; 4) longue écriture séquentielle (1, 3 ou 10 min) et courbe du débit, chute du cache SLC repérée ; 5) lecture aléatoire sur les données écrites. Puis premier et dernier Mio remis à zéro (clé vide). Testé sur une clé simulée, contrefaçons comprises |
| `diagnostic/analyse.rs` (`analyser_espace_libre`) | mesure d'écriture **sans rien effacer** : fichier d'essai dans le volume monté le plus spacieux de la clé (`disques::emplacements` : mountinfo + `slaves` sous Linux, à travers LUKS/LVM ; lettres de volume sous Windows), au plus 90 % de l'espace libre moins 512 Mio. Linux : fichier retiré du dossier dès son ouverture (`unlink`) ; Windows : `FILE_FLAG_DELETE_ON_CLOSE` — disparu même après un arrêt brutal. Zone de travail de 256 Mio, écritures 4 Kio (1 puis 8), synchronisées, lectures, puis longue écriture. Pas de contrôle de capacité. Sans droits administrateur sous Linux |
| Clé système (Linux) | `lister_systeme_usb` : la clé USB qui porte le système en cours est proposée **dans l'onglet Diagnostic seulement** (« système en cours »), pour la lecture et l'espace libre ; `retrouver_diagnostic` l'accepte, `retrouver` (fabrication, analyse complète) jamais |
| `diagnostic/interpretation.rs` | fiches par facette, dans l'ordre d'importance, chacune **lu / mesuré / déduit / non mesuré** ; estimation prudente de la catégorie A–D ; résumé copiable. Seuils indicatifs, à calibrer sur des clés réelles |
| `diagnostic/textes.rs` | textes explicatifs de `ressources/diagnostic/textes.txt` (`[clé]` et `[clé.pourquoi]`), **modifiables sans toucher au code** ; un test vérifie que chaque clé utilisée existe |

Sous Linux, les mesures passent par l'assistant : `pkexec <app> --diagnostic` (JSON du `Disque` en
entrée, JSON des `Mesures` dans `FINI`) et `pkexec <app> --analyse` (disque + réglages, JSON de
l'`Analyse`). La clé est revérifiée (liste USB hors système) côté root. Sans pkexec (paquet séparé
sous Debian, absent du système Fabrique-Clé), l'application se rabat sur `run0` (systemd ≥ 256,
polkit), sinon demande d'installer pkexec.

## 5. CI (`.github/workflows/image.yml`, `app.yml`)

`app.yml` (push sur `app/**`, sur `main` et les branches `claude/**`) : tests + compilation Linux
(ubuntu-22.04, binaire + AppImage) et Windows (`.exe`), en artefacts 14 jours ; sur étiquette `v*`,
joints à la release de l'image (`fabrique-cle-windows.exe`, `fabrique-cle-linux-x86_64.AppImage`).

`image.yml` : push sur `image/**` : construction → test QEMU → compression zstd (`-12 --long=27`) + bmap + sha256 → artefact 7 jours.
Garde-fou : échec si l'image compressée dépasse 3 Go (signe d'un volume chiffré dans l'image ou de blocs
libres « sales »). Découpage en `*.part00…` au-delà de 2 000 Mio (limite GitHub : 2 Gio par fichier).
Étiquette `v*` : idem + release GitHub. Le test démarre une **copie agrandie de 6 Gio** de l'image en
**clé USB xHCI**, OVMF **Secure Boot avec clés Microsoft**, NVRAM vierge, puis inspecte la clé hors ligne
(15 vérifications, dont « AES-XTS, plus de `cipher_null` »). En cas d'échec : capture d'écran `ecran.ppm` et journal en artefact.
