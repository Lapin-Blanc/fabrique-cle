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
- RTC en heure locale pour cohabiter avec Windows sans décalage d'heure. Revers : sur un PC Linux
  (RTC en UTC, ex. Zorin), la clé démarre avec une heure de décalage (constaté : 1 h de retard en
  hiver). D'où **`systemd-timesyncd`** (absent de la variante `important` de mmdebstrap : « NTP
  service: n/a ») : l'heure se corrige dès que la clé est en ligne. Avec la RTC en heure locale,
  systemd n'écrit jamais l'horloge matérielle : l'heure du système hôte n'est pas modifiée.

## Premier démarrage (écran)

- **Questions invisibles, écran figé** (vrai PC, 2026-11-01) : le service démarrait tôt, en parallèle ;
  sa fenêtre whiptail était aussitôt recouverte par les lignes « Started … » de systemd et les messages
  du noyau. L'élève voyait un démarrage arrêté alors qu'une question (phrase secrète) attendait.
  Correctifs : service `After=plymouth-quit-wait.service systemd-user-sessions.service`,
  `plymouth quit`, puis dans le script `dmesg -n 1`, `kill -s RTMIN+21 1` (systemd n'affiche plus
  l'état), `chvt 1`, `clear`.
- **Cause principale : `StandardError=journal`.** L'idiome `x=$(whiptail … 3>&1 1>&2 2>&3)` envoie
  l'interface de whiptail sur la sortie d'erreur : elle était écrite dans le journal, jamais à l'écran.
  Le service doit avoir `StandardError=tty`. (Révélé par la capture du test : écran propre, sans fenêtre.)
- Le test CI passait car il fournissait la phrase dans `config.txt` (aucune question affichée). Il la
  tape désormais au clavier virtuel (`sendkey`), seulement quand la capture d'écran est majoritairement
  bleue (fond whiptail) : une question invisible fait échouer le test.

## Clé lente (USB sans UAS) — essai du 2026-11-01 (SanDisk 3.2, `usb-storage`, 13 Mo/s en écriture)

- Mesures (passation de la session menée sur la clé) : lecture 150 Mo/s (4k : 16 Mo/s), écriture
  13 Mo/s (4k synchrone : 0,4 Mo/s, ~100 IOPS), `nr_requests=2`, pas de TRIM, vue `rotational=1`.
  ~10 % du temps bloqué en E/S avant réglages, ~3 % après. Démarrage 45,8 s (firmware 11,7 · GRUB 12,3
  · noyau + saisie LUKS 15,8 · espace utilisateur 6,0).
- Le frein est l'écriture : une requête à la fois, 0,4 Mo/s en petits blocs. Linux accumulait ~1,4 Go
  à écrire et le bureau se figeait. Écriture différée bornée (`vm.dirty_*_bytes`) : plus de gel.
- Règle udev BFQ : numérotée **61**, sinon elle passe avant `60-persistent-storage` qui définit `ID_BUS`.
- `~/.cache` en tmpfs suffit pour les caches de navigateur. **profile-sync-daemon écarté** : jusqu'à
  1 h de profil perdue si la clé est arrachée, et il faut le maintenir par navigateur.
- `commit=30` plutôt que 60 : compromis entre écritures et données perdues en cas d'arrachage.
- Démarrage : 12 s dans GRUB à charger un `initrd` de 133 Mo depuis la clé lente. Mesure (264 Mo non
  compressés) : firmwares graphiques ~162 Mo (amdgpu 80, nvidia 63, i915 11…), pilotes graphiques 10 Mo,
  réseau ~15 Mo avec firmwares, microcode 15 Mo. Le hook `zz-fabrique-cle-allege` retire `drivers/gpu`,
  `drivers/net` et les firmwares qu'eux seuls demandent (`modinfo -F firmware`) ; la question LUKS
  s'affiche via simpledrm (intégré au noyau). Garde-fous à la construction (taille ≤ 70 Mo, dm-crypt,
  usb-storage, uas, xhci, hid présents) et deuxième démarrage testé jusqu'à SDDM.

- Exclure un navigateur ne suffit pas : `webext-plasma-browser-integration` (recommandé par Plasma)
  recommande « firefox-esr | firefox | chromium » ; exclure Firefox ferait prendre Chromium.
  Le garde-fou de `construire-image.sh` l'a détecté (Firefox installé malgré son retrait de la liste).
- `fstrim.timer` conservé : sans effet sur une clé `usb-storage`, utile sur un SSD USB en UAS.

## Chiffrement

- Clavier : sans `KEYMAP=y` (+ `setupcon --save-only`), la phrase LUKS se tape en QWERTY. Validé par
  déverrouillage avec une phrase de test dont les lettres diffèrent entre AZERTY et QWERTY.
- **`cryptsetup reencrypt --batch-mode` n'affiche aucune progression** (`utils_progress.c` : la ligne
  « Progress: … » est sautée en mode batch). `--progress-json` est, lui, affiché même en batch, sur la
  sortie standard : une ligne JSON par intervalle (`device_bytes`, `device_size`, `speed`, `eta_ms` ; les
  valeurs sont des chaînes). Au premier appel le débit est nul et `eta_ms` vaut n'importe quoi (division
  par zéro côté C) : ne l'afficher que si le débit est positif. À la reprise (`--resume-only`),
  `device_bytes` est la position absolue (`rh->progress = rh->offset`) : la jauge repart du bon endroit.
- Jauge dans un tube (`cryptsetup | filtre | whiptail --gauge`) : si l'affichage meurt, SIGPIPE
  interromprait le chiffrement → le filtre ignore SIGPIPE et lit tout ; `whiptail … || true`.
- **whiptail tronque sans prévenir** (vu sur la capture `jauge.ppm` de la CI, reproduit avec un émulateur
  de terminal `pyte`) : `--gauge` fixe la hauteur de sa zone de texte d'après le texte **initial**, et
  les mises à jour `XXX` n'ajoutent jamais de lignes → texte initial bourré de lignes vides. La hauteur
  est calculée **avant** le repli des lignes longues : une ligne repliée fait disparaître la dernière →
  lignes de moins de 70 caractères. `--infobox` perd ses dernières lignes si la boîte n'est pas assez
  haute (5 lignes de texte : hauteur 12, pas 10).
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

- Téléchargement par morceaux (`Range`) : GitHub le permet, y compris après la redirection vers
  `release-assets.githubusercontent.com` (vérifié avec curl : 206). Repli automatique sur un seul
  flux si la sonde ou tous les morceaux échouent (proxy, pare-feu). Le proxy des sessions Claude
  Code dans le nuage casse les réponses de GitHub pour `ureq` (« chunk expected crlf ») : à tester
  sur un vrai PC (`cargo test morceau_reel -- --ignored`).

## Essais de distributions (historique)

- PorteuX 2.8 COSMIC : fenêtres et menus qui ne s'ouvraient pas → passage au module KDE, fonctionnel.
- elementary OS 8.1 : fonctionnel après les deux correctifs EFI ci-dessus.
- Debian 13 + Plasma minimal (installation par préconfiguration dans une VM libvirt, clé en disque brut) :
  meilleur résultat → base de ce projet. Scripts d'origine : `image/scripts/postinstall-reference.sh`.
