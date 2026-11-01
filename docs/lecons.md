# Leçons tirées des essais

## Démarrage UEFI d'une clé nomade

- **`fbx64.efi` dans `EFI/BOOT` = boucle de redémarrage.** Le shim, lancé par le chemin amovible, voit
  `fbx64.efi` à côté de lui, tente de créer une entrée NVRAM puis redémarre : écran noir, un ou deux
  redémarrages, puis le PC démarre sur son propre système. Ne jamais laisser `fbx64.efi` ni `BOOTX64.CSV`
  dans `EFI/BOOT` (hook APT `99-no-shim-fallback`).
- **GRUB « variante CD » dans `EFI/BOOT` = invite `grub>`.** Sous Ubuntu/elementary, `grub-install --removable`
  y a placé un `grubx64.efi` qui cherche sa config ailleurs. Vérifier que `EFI/BOOT/grubx64.efi` est
  identique à `EFI/<distro>/grubx64.efi` (contrôle dans le script de construction).
- Ne jamais écrire dans la NVRAM : `--no-nvram`, `update_nvram=false`, pas d'`efibootmgr`. Tester avec une
  NVRAM OVMF **vierge** à chaque fois.
- Secure Boot : shim signé Microsoft + GRUB et noyau signés Debian ; vérifié dans OVMF avec
  `OVMF_CODE_4M.ms.fd`/`secboot.fd` + `OVMF_VARS_4M.ms.fd` (SMM obligatoire).

## Matériel

- **SSD SSK (USB 090c:2320) instable en UAS** sur le portable HP ProBook 455 de l'enseignant (déconnexions
  pendant mkfs). Contournement côté hôte : quirk `usb-storage` `090c:2320:u` (perd le TRIM). Sur la clé
  elle-même, rien n'est forcé : en UAS le TRIM fonctionne (`lsblk --discard`). À surveiller : erreurs
  `uas_eh_abort_handler` dans `journalctl -k`.
- Matériel inconnu : `MODULES=most` dans l'initramfs (sinon la clé ne trouve pas son propre disque sur
  certains contrôleurs USB/NVMe).
- RTC en heure locale pour cohabiter avec Windows sans décalage d'heure.

## Chiffrement

- Clavier : sans `KEYMAP=y` (+ `setupcon --save-only`), la phrase LUKS se tape en QWERTY. Validé par
  déverrouillage avec une phrase de test dont les lettres diffèrent entre AZERTY et QWERTY.
- Une image publiée a une clé maîtresse publique → `cryptsetup reencrypt` obligatoire sur chaque clé.
  Le faire **avant** l'agrandissement (≈ 1 min au lieu de dizaines).
- **Un volume chiffré dans l'image la rend incompressible** (7,5 Go compressés pour 5,2 Go de système,
  et `fstrim` à travers dm-crypt ne libère pas tout). Solution : image en LUKS2 **`cipher_null`**
  (contenu en clair), chiffrée en AES-XTS au premier démarrage par le même `reencrypt` en ligne, en
  précisant `--cipher` (par défaut, `reencrypt` garde le chiffrement actuel, donc `cipher_null` !).
  Écarté : `reencrypt --encrypt --reduce-device-size` (décalage des données, à faire hors ligne dans
  l'initramfs : bien plus complexe).
- **`.deb` téléchargés dans l'image = ~1,4 Gio de blocs libres incompressibles** : après `apt-get clean`,
  `fstrim` (à travers LVM et dm-crypt) n'a rendu au fichier que 1,7 Gio sur ~3,1 Gio libres. Cache APT
  monté depuis l'hôte pendant la construction : 3 Gio rendus, 7 168 → 5 833 Mio alloués, 3,47 → 2,06 Go
  compressés. Le script affiche « Blocs alloués dans le fichier image » : à surveiller.
- **`fstrim` sur système monté : résultat variable** (2,8 à 3 Gio rendus selon les builds, ±90 Mo
  compressés). `e2fsck -f -E discard` après démontage est exhaustif et reproductible.
- Ce qui pèse dans l'image (mesuré en montant l'ext4 de l'image `cipher_null` via un loop décalé, sans
  device-mapper) : bibliothèques 1,5 Gio, **polices 791 Mio** (dont ~600 Mio de graisses « extra »
  Noto tirées par les recommandations de `plasma-desktop`), firmwares 552 Mio, traductions 512 Mio,
  Firefox 295 Mio. Contenu largement déjà compressé (modules `.ko.xz`, firmwares `.zst`) : ratio ~2,8×.
- zstd sur l'image (5,8 Gio) : `-12` 72 s ; `-12 --long=27` −5 % en 77 s ; `-17 --long=27` −8,7 % mais 451 s.
- Avec `set -o pipefail`, éviter `cryptsetup luksDump | grep -q` (SIGPIPE possible) : lire l'en-tête
  dans une variable.
- `cryptsetup resize` après suppression de la clé temporaire peut demander un secret : agrandir **avant**
  de retirer la clé temporaire.
- Dans un chroot, le hook cryptsetup ne peut pas deviner la racine : option `initramfs` dans crypttab.

## Construction de l'image (CI)

- `readlink -f` échoue si le dossier parent n'existe pas : utiliser `realpath -m`.
- dpkg pose une question bloquante si un fichier de configuration du paquet existe déjà
  (`conf-hook`) : `-o Dpkg::Options::=--force-confold`.
- **Montages propagés aux services de l'hôte** (espaces de montage des démons systemd) : le volume reste
  « en cours d'utilisation » après `umount`. Solution : tout le script dans `unshare --mount --propagation private`.
  Tuer aussi les processus restés dans le chroot avant de démonter.
- `lsblk -no PKNAME <partition>` liste aussi les enfants (dm-0…) : utiliser `lsblk -dno PKNAME`.
- Le test doit s'éteindre tout seul même en cas d'échec (sinon 30 min de délai perdues) et rapporter le journal.

## Application

- **Sel SHA-512 crypt de 16 caractères maximum.** La crate `sha-crypt` (0.6) encode par défaut un sel
  de 22 caractères ; la glibc le tronque à 16 et renvoie une chaîne différente de celle stockée :
  connexion impossible. Sel de 12 octets (= 16 caractères) ; vérifié avec `crypt(3)` de la glibc.
- `bmaptool create` cite des balises (`<BlocksCount>`…) dans ses commentaires XML : les retirer avant analyse.
- Windows : lectures/écritures d'un disque brut alignées sur le secteur → `fatfs` passe par une fenêtre
  à blocs de 4096 octets (`esp.rs`).
- AppImage + pkexec : root ne peut pas lire le montage FUSE de l'AppImage → relancer `$APPIMAGE`.
- Windows : une application élevée (administrateur) ne reçoit pas le glisser-déposer de l'Explorateur
  (UIPI) → bouton « Parcourir… » (crate `rfd`).

## Essais de distributions (historique)

- PorteuX 2.8 COSMIC : fenêtres et menus qui ne s'ouvraient pas → passage au module KDE, fonctionnel.
- elementary OS 8.1 : fonctionnel après les deux correctifs EFI ci-dessus.
- Debian 13 + Plasma minimal (installation par préconfiguration dans une VM libvirt, clé en disque brut) :
  meilleur résultat → base de ce projet. Scripts d'origine : `image/scripts/postinstall-reference.sh`.
