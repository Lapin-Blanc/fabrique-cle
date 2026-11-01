# Analyse fonctionnelle

## Contexte

Un enseignant a mis au point, par essais successifs sur un SSD USB (SSK 1 To), une clé Debian 13 + KDE Plasma
« nomade » : elle démarre sur n'importe quel PC UEFI, Secure Boot actif, sans créer d'entrée dans la NVRAM du PC
ni toucher à son disque, avec le système chiffré. Le résultat est jugé le meilleur parmi les essais
(PorteuX COSMIC/KDE, elementary OS 8.1, Debian 13). Objectif : permettre aux **élèves** de se fabriquer une
telle clé eux-mêmes, avec quelques personnalisations.

## Utilisateurs

| Qui | Contexte | Attentes |
|---|---|---|
| Élève | PC personnel **Windows** ou **Linux**, droits administrateur, peu d'expérience | un exécutable, quelques champs, un bouton, une clé qui marche |
| Enseignant (mainteneur) | Linux (Zorin OS), GitHub | publier une nouvelle image sans manipulations manuelles ; ajuster le contenu de l'image |

## Parcours de l'élève

1. Télécharge l'application (release GitHub : `.exe` Windows ou AppImage Linux).
2. Branche une clé USB (≥ 16 Go ; SSD USB recommandé).
3. Choisit la clé (seuls les disques USB amovibles sont proposés), la version de l'image, et remplit :
   identifiant, nom complet, mot de passe, nom de machine, clavier, langue, chiffrement oui/non.
4. Confirme l'effacement ; l'application télécharge l'image (avec vérification SHA-256), l'écrit, dépose
   `fabrique-cle/config.txt` sur la partition EFI.
5. Démarre un PC sur la clé (menu de démarrage du firmware, ex. F9 chez HP, F12 ailleurs).
6. Premier démarrage : écran texte ; si chiffrement, saisit **deux fois sa phrase secrète** ; patiente
   (~2 min) ; arrive sur l'écran de connexion Plasma.

L'image doit **aussi** fonctionner gravée avec un autre outil (Rufus, Etcher, dd) : sans `config.txt`,
le premier démarrage pose toutes les questions à l'écran.

## Exigences fonctionnelles

- F1 Écrire l'image sur une clé USB choisie, sans jamais proposer le disque système.
- F2 Personnaliser : identifiant, nom complet, mot de passe, nom de machine, clavier (be, fr, ch, us),
  langue (fr_BE, fr_FR, fr_CH, nl_BE, en_US), chiffrement.
- F3 Utiliser toute la capacité de la clé (agrandissement automatique au premier démarrage).
- F4 Chiffrement LUKS2 avec une clé maîtresse **propre à chaque clé** (l'image publiée est publique).
- F5 Démarrer sur tout PC UEFI x86-64, Secure Boot actif ou non, sans écrire dans sa NVRAM.
- F6 Matériel inconnu : tous les pilotes dans l'initramfs, firmwares Wi-Fi/GPU/son courants.
- F7 Clavier choisi actif dès la saisie de la phrase LUKS.
- F8 Plus tard (optionnel) : paquets supplémentaires cochables (programmation, bureautique…).

## Exigences non fonctionnelles

- Sécurité : aucun mot de passe en clair écrit sur la clé ; mot de passe transmis haché (SHA-512 crypt).
- Fiabilité : chaque image publiée a passé le test automatique (QEMU, Secure Boot, clé USB, premier démarrage chiffré).
- Simplicité : un exécutable sans installation ; aucune dépendance côté élève.
- Coût : hébergement gratuit (GitHub public : Actions et releases).
- Robustesse : premier démarrage rejouable après coupure de courant.

## Hors périmètre (pour l'instant)

- macOS comme poste de fabrication ; PC sans UEFI (BIOS legacy) ; ARM.
- Gestion de flotte (création en masse par l'enseignant) — l'architecture la permet plus tard.
