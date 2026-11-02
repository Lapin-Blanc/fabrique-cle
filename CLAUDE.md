# CLAUDE.md — Fabrique-Clé

Projet de Fabien Toune (enseignant, EICA, Belgique). Langue de travail : **français** (code commenté en
français, messages de commit en français). Utilisateur francophone, clavier **belge AZERTY**.

## Ce qu'est le projet

Un outil fenêtré (style Rufus, en Rust) que les élèves téléchargent pour se fabriquer eux-mêmes une clé USB
amorçable **Debian 13 + KDE Plasma minimal**, chiffrée, qui démarre sur n'importe quel PC UEFI (Secure Boot
compris) sans rien modifier sur ce PC. Lire dans l'ordre :

1. `docs/analyse-fonctionnelle.md` — besoins, utilisateurs, parcours, exigences
2. `docs/architecture.md` — image, premier démarrage, application, CI
3. `docs/lecons.md` — **pièges déjà rencontrés : à lire avant de toucher au démarrage, à GRUB ou au chiffrement**
4. `docs/feuille-de-route.md` — état d'avancement et prochaines étapes

## Organisation

- `image/construire-image.sh` — construit l'image (root, Linux, mmdebstrap, loop) ; `image/fichiers/` est copié tel quel dans l'image
- `image/fichiers/usr/local/sbin/fabrique-cle-premier-demarrage` — service de premier démarrage (tty1, avant SDDM)
- `image/tester-image.sh` — test QEMU/OVMF Secure Boot + clé USB + inspection hors ligne
- `image/config-exemple.txt` — format du fichier de personnalisation (contrat entre l'application et l'image)
- `app/` — application Rust egui (Windows + Linux) : `cargo test` dans `app/` ; modules décrits dans `architecture.md` § 4
- `.github/workflows/image.yml` — construction + test à chaque push sur `image/**`, release sur étiquette `v*`
- `.github/workflows/app.yml` — tests + `.exe` Windows + AppImage Linux à chaque push sur `app/**`, joints à la release sur `v*`
- `.github/workflows/publier.yml` — **publier une version** : lancé à la demande (entrée `version`, ex. `v2026.11.2`),
  après **approbation de l'enseignant** (environnement GitHub `release`) ; pose l'étiquette sur `main`, crée la
  release en pré-version, appelle `image.yml` et `app.yml`. Claude Code le lance avec `actions_run_trigger`
  (`run_workflow`, `ref: main`) — il ne peut pas pousser d'étiquette lui-même.

## Règles

- Ne jamais toucher à un disque réel sans identification par numéro de série et confirmation explicite de l'utilisateur.
  L'image se construit et se teste **dans des fichiers**, jamais sur une clé.
- Ne jamais écrire dans la NVRAM UEFI (pas d'`efibootmgr`, `update_nvram=false`, `--no-nvram`).
- Toute modification de l'image doit passer le test CI (`tester-image.sh`) avant d'être considérée comme faite.
- Les scripts restent idempotents et commentés ; le service de premier démarrage doit pouvoir être relancé après une coupure.
- Aucun secret en clair dans le dépôt ni dans `config.txt` (mot de passe haché ; phrase LUKS saisie au premier démarrage).
- Commits : identité déjà configurée dans le dépôt (adresse noreply GitHub de Toune-Fabien).

## Construire / tester

- En CI : pousser sur `main` (chemins `image/**`), ou lancer `image.yml` à la demande sur une branche ;
  suivre avec `gh run watch`. Diagnostic en artefact (`ecran.ppm`, journal).
- En local (Linux, root) : `sudo image/construire-image.sh` puis `sudo image/tester-image.sh`
  (paquets : mmdebstrap gdisk cryptsetup lvm2 dosfstools qemu-system-x86 ovmf zstd bmap-tools cloud-guest-utils socat).
- Un build complet + test prend ~9 min sur un runner GitHub.
