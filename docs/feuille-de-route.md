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
- [ ] Première release `v2026.11`

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
graisses supplémentaires des polices Noto) : **1 893 Mo** (run 41304716205), puis `zstd --long=27`
(≈ −5 %). Marge d'environ 300 Mo sous le seuil de découpage. Piste suivante si besoin : traductions
(`/usr/share/locale`, 512 Mio, 214 langues) limitées aux langues proposées — à décider.

## Prochaines étapes

### Image
- [ ] Test d'un **deuxième démarrage** jusqu'à SDDM (phrase LUKS tapée via QMP `send-key`, comme lors de
      l'installation manuelle), et du parcours **interactif** sans config.txt.
- [ ] Test sans chiffrement (`chiffrer=non`).
- [ ] Vérifier la taille de la clé minimale (16 Go) ; garder l'image compressée sous 2 000 Mio.
- [ ] Essai sur vrai matériel (portable HP, PC de l'école) : Wi-Fi, son, veille, `lsblk --discard`.
- [ ] Mises à jour : automatiques (unattended-upgrades) ? à décider.
- [ ] Options de logiciels supplémentaires (F8) : installées au 1er démarrage si réseau, ou variantes d'image.

### Application Rust (`app/`)
- [ ] Squelette `eframe`/`egui`, fenêtre style Rufus (maquette dans `architecture.md` / discussion)
- [ ] Énumération des disques USB (Linux : udisks2/lsblk ; Windows : SetupAPI/WMI) avec garde-fous
- [ ] Téléchargement + zstd en flux + bmap + SHA-256, barre de progression
- [ ] Écriture brute (Linux udisks2/pkexec ; Windows PhysicalDrive + verrouillage des volumes)
- [ ] Écriture de `config.txt` dans la FAT de p1 (crate `fatfs`) ; hachage `sha-crypt`
- [ ] Workflow `app.yml` : builds Windows (`.exe`) et Linux (AppImage), attachés à la release
- [ ] Signature de code Windows (SmartScreen) : à évaluer (payant)

## Limites connues de l'environnement cloud

Le développement de l'application (Rust) et des scripts peut se faire dans Claude Code sur le web. La
construction/le test de l'image demandent root + loop + KVM : ils tournent dans **GitHub Actions**.
L'essai de l'application sur une vraie clé USB se fait forcément sur un PC local.
