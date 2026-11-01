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
- [ ] Étape de compression/artefact du workflow (corrigée, à confirmer)
- [ ] Première release `v2026.11` et vérification de la taille compressée (limite 2 Gio par fichier)

## Prochaines étapes

### Image
- [ ] Test d'un **deuxième démarrage** jusqu'à SDDM (phrase LUKS tapée via QMP `send-key`, comme lors de
      l'installation manuelle), et du parcours **interactif** sans config.txt.
- [ ] Test sans chiffrement (`chiffrer=non`).
- [ ] Vérifier la taille de la clé minimale (16 Go) et réduire l'image si possible.
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
