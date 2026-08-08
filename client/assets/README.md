# Assets locaux

**Rien dans ce dossier n'est versionné** (voir `.gitignore`), à l'exception de
ce fichier.

## Pourquoi

Le dépôt est public et sous AGPL-3.0. Y committer des fichiers dont le projet
n'a pas les droits, c'est de la redistribution, pas un usage privé — et
l'historique git les conserve même après suppression, ce qui obligerait à le
réécrire en entier pour les faire disparaître.

Poser des fichiers ici les garde sur la machine et hors du dépôt. Ce que
chacun utilise localement le regarde ; ce que le dépôt distribue engage le
projet.

## Organisation attendue

```
client/assets/
├── models/      personnages, créatures  (.glb, .gltf)
├── textures/    (.png, .jpg)
├── ui/          icônes, cadres
└── sounds/      (.ogg, .wav)
```

Le client fonctionne **sans aucun de ces fichiers** : il affiche alors des
formes géométriques. Les assets améliorent le rendu, ils ne conditionnent pas
le jeu.

## Si le projet doit un jour distribuer des assets

Il lui en faudra dont il a les droits. Trois voies, détaillées dans
[la roadmap](../../docs/ROADMAP.md) :

- **CC0** (Kenney, Quaternius, Mixamo) — gratuit, style hétérogène
- **packs achetés** — vérifier que la licence couvre un usage en ligne
- **production maison** — la seule voie vers une identité propre

Ces assets-là iraient dans un dossier versionné, distinct de celui-ci, avec
leurs licences.
