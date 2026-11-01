# Fabrique-Clé

Outil pour préparer soi-même une clé USB amorçable avec **Debian 13 + KDE Plasma (léger)**,
utilisable sur n'importe quel PC UEFI (Secure Boot compris), sans toucher au PC hôte.

- `image/` : construction de l'image système (Linux, root) — exécutée par GitHub Actions
- `app/` : application graphique Rust (Windows et Linux) qui écrit l'image et la personnalise
- `.github/workflows/` : construction + test (QEMU/OVMF) + publication des releases
- `docs/` : notes de conception

## Principe

1. GitHub Actions construit l'image (`mmdebstrap`, LUKS2 + LVM, GRUB amovible, Plasma minimal),
   la teste dans QEMU (NVRAM vierge, Secure Boot) et la publie dans une release.
2. L'application télécharge l'image, l'écrit sur la clé choisie et dépose un fichier de
   personnalisation sur la partition EFI.
3. Au premier démarrage, la clé applique la personnalisation (compte, nom de machine, clavier),
   s'agrandit à sa taille réelle et, si demandé, remplace la clé de chiffrement temporaire.

## Documentation

- [Analyse fonctionnelle](docs/analyse-fonctionnelle.md)
- [Architecture](docs/architecture.md)
- [Leçons tirées des essais](docs/lecons.md)
- [Feuille de route](docs/feuille-de-route.md)
