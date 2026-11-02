# Feuille de route

## Stratégie

1. D'abord l'**image** et son **premier démarrage**, testés automatiquement : c'est le cœur, l'application
   n'est ensuite qu'un graveur avec un formulaire.
2. Puis l'**application Rust**, d'abord sous Linux, puis Windows ; compilée par GitHub Actions.
3. Valider sur **vrai matériel** (le portable de l'enseignant, puis des PC de l'école) avant diffusion.
4. Publier par étiquettes `vAAAA.MM` (image + exécutables dans la même release).

## État (2026-11-01)

- [x] Dépôt public `Toune-Fabien/fabrique-cle`
- [x] `construire-image.sh` : image 10 Gio, ~5,2 Go utilisés, construite en ~6 min sur GitHub
- [x] Service de premier démarrage (config.txt ou questions, rechiffrement, agrandissement, compte)
- [x] `tester-image.sh` : test complet réussi (Secure Boot, USB, NVRAM vierge, 14 vérifications)
- [x] Workflow complet vert (construction, test, compression, artefact) — run 41296826593, 11 min 34 s
- [x] Première release `v2026.11` (pré-version ; les releases sont créées en pré-version, à promouvoir
      après essai sur vrai matériel)

## Taille de l'image compressée — traité (7,5 Go → < 1,9 Go, un seul fichier)

Constaté le 2026-11-01 : `fabrique-cle.img.zst` = 7,5 Go alors que le système occupe 5,2 Go. Cause : le
volume est **chiffré dans l'image**, donc incompressible ; et une partie de l'espace libre chiffré n'est
pas rendue creuse par `fstrim`. Trop lourd pour des élèves.

Piste recommandée : publier l'image **non chiffrée** (LVM sur p3 en clair, compressible ~2-2,5×, donc
~2 Go) et **chiffrer au premier démarrage** avec `cryptsetup reencrypt --encrypt --reduce-device-size 32M`
(chiffrement en place de LUKS2). Le chiffrement en place d'un volume racine monté n'est pas possible en
ligne : il faut le faire **dans l'initramfs** (script `local-top` avant le montage de la racine, déclenché
par un marqueur posé par le service de premier démarrage, puis redémarrage). Bonus : plus besoin de clé
temporaire ni de rechiffrement. À prototyper et à couvrir par `tester-image.sh`.

**Solution retenue (2026-11-01)**, plus simple que la piste initramfs : le volume LUKS2 de l'image est
créé en **`cipher_null`** (données en clair, donc compressibles), et le service de premier démarrage le
chiffre **en ligne** en AES-XTS avec `cryptsetup reencrypt --cipher aes-xts-plain64 --key-size 512` —
le même mécanisme que le rechiffrement déjà testé, sans décalage de données ni script initramfs.
Le test vérifie qu'il ne reste aucun segment `cipher_null` (15 vérifications, runs 41300324296 et 41301253775 verts ; un seul fichier `.zst` de 2 055 Mo ;
chiffrement en ligne ≈ 1 min 40 dans QEMU).

Mesures (zstd -12) : volume chiffré 7,5 Go → `cipher_null` 3,47 Go → + cache APT hors de l'image
**2,06 Go**, sous la limite de 2 Gio par fichier de release (seuil de découpage porté à 2 000 Mio).
Le contenu lui-même (modules `.ko.xz`, firmwares `.zst`, Firefox) est déjà compressé : ratio ~2,8× sur
les 5,8 Gio alloués. La CI échoue au-delà de 3 Go.

Deuxième passe : le build suivant sur `main` donnait 2 143 Mo (deux morceaux) à cause de `fstrim`,
variable. Corrigé par `e2fsck -E discard` au démontage ; allègement sans toucher aux firmwares (doc, man,
graisses supplémentaires des polices Noto) : 1 893 Mo (run 41304716205), puis `zstd --long=27` :
**1 796 Mo** (run 41305755188 ; 5 185 Mio alloués, identiques d'un build à l'autre). Marge d'environ
300 Mo sous le seuil de découpage. Piste suivante si besoin : traductions
(`/usr/share/locale`, 512 Mio, 214 langues) limitées aux langues proposées — à décider.

## Prochaines étapes

### Image
- [x] Phrase secrète tapée au clavier virtuel par le test, question réellement visible (fond bleu détecté)
- [x] Test d'un **deuxième démarrage** jusqu'à SDDM (phrase LUKS tapée au clavier virtuel)
- [ ] Test du parcours **interactif** sans config.txt (toutes les questions à l'écran).
- [ ] Test sans chiffrement (`chiffrer=non`).
- [ ] Vérifier la taille de la clé minimale (16 Go) ; garder l'image compressée sous 2 000 Mio.
- [ ] Essai sur vrai matériel (portable HP, PC de l'école) : Wi-Fi, son, veille, `lsblk --discard`.
- [x] Mises à jour : aucune automatique ; Discover les signale (décision du 2026-11-02)
- [x] Brave seul navigateur ; fond « image du jour » ; réglages pour clé lente (voir `lecons.md`)
- [x] `initrd` allégé (sans pilotes graphiques ni réseau ni leurs firmwares) : à mesurer sur clé lente
- [ ] Vérifier sur une vraie session Plasma : fond « image du jour » appliqué, Brave par défaut, BFQ actif
- [ ] Options de logiciels supplémentaires (F8) : installées au 1er démarrage si réseau, ou variantes d'image.

### Application Rust (`app/`)
- [x] Fenêtre `eframe`/`egui` style Rufus (clé, image, personnalisation, état, confirmation)
- [x] Énumération des disques USB avec garde-fous (Linux : sysfs ; Windows : IOCTL sur PhysicalDrive)
- [x] Téléchargement (cache, morceaux, SHA-256) + zstd en flux + bmap + vérification par relecture
- [x] Écriture brute (Linux : assistant pkexec ; Windows : PhysicalDrive + verrouillage des volumes)
- [x] Écriture de `config.txt` dans la FAT de p1 (crate `fatfs`) ; hachage `sha-crypt` (compatible glibc)
- [x] Workflow `app.yml` : builds Windows (`.exe`) et Linux (AppImage), attachés à la release
- [x] Essai bout en bout sur vraie clé (SSD SSK USB) : écriture par l'application → premier démarrage
      (config.txt lu, questions à l'écran) → clé fonctionnelle sur le HP ProBook 455 — 2026-11-01
- [ ] Vérifier explicitement les deux plateformes d'écriture : Windows (UAC, verrouillage des volumes)
      et Linux (Zorin, assistant pkexec)
- [x] Apparence soignée (police Inter, thèmes clair/sombre, cartes), icône de fenêtre
- [x] Téléchargement parallèle avec reprise après coupure (repli sur un seul flux)
- [x] Onglet **Diagnostic**, étape 1 (lecture seule) : identification de la clé, mesures en lecture
      (séquentielle, aléatoire 4 Ko une et 8 à la fois, latences), fiches explicatives — 2026-11-02
- [ ] Diagnostic sur vraies clés (Linux et Windows) : SanDisk 3.2 de l'essai du 01/11, SSD SSK, une clé
      bas de gamme ; calibrer les seuils de `interpretation.rs`
- [x] Diagnostic, étape 2 : analyse complète **destructive** (confirmation par n° de série) : capacité
      réelle (marqueurs), écriture aléatoire 4 Ko, écritures synchronisées, longue écriture et courbe
      de débit (cache SLC), lecture sur données écrites ; catégorie A–D mesurée — 2026-11-02
- [x] Diagnostic : mesure d'écriture sur l'espace libre (fichier d'essai, sans effacer) ; clé système en
      cours proposée au diagnostic (Linux) — 2026-11-02
- [x] Essais réels sous Windows : SSD WD SN350 en boîtier RTL9210 (lectures irrégulières → fiche
      « Régularité »), SanDisk 3.2 Gen 1 (BOT, lectures régulières ~2 000/s, catégorie A/B en lecture)
- [ ] Vérifier le repli `run0` (application lancée depuis une clé Fabrique-Clé, sans pkexec)
- [ ] Diagnostic, étape 3 : rapport exporté (texte/Markdown) pour comparer des modèles
- [ ] Icône de l'exécutable Windows (ressource .ico)
- [ ] Signature de code Windows (SmartScreen) : à évaluer (payant)

## Limites connues de l'environnement cloud

Le développement de l'application (Rust) et des scripts peut se faire dans Claude Code sur le web. La
construction/le test de l'image demandent root + loop + KVM : ils tournent dans **GitHub Actions**.
L'essai de l'application sur une vraie clé USB se fait forcément sur un PC local.
