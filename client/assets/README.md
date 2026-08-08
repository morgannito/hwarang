# Assets locaux

**Rien dans ce dossier n'est versionné** (voir `.gitignore`), sauf ce fichier.

Le client fonctionne **sans aucun fichier ici** : il affiche alors des disques —
bleus pour les joueurs, rouges pour les créatures, cerclés de blanc pour
soi-même. Les assets améliorent le rendu, ils ne conditionnent pas le jeu.

## Où poser quoi

```
client/assets/
├── textures/
│   ├── player.dds            silhouette du joueur
│   ├── player_weapon_1.dds   variante quand l'objet 1 est équipé
│   ├── player_weapon.dds     repli pour toute arme
│   └── creature.dds          créatures
└── ground/
    └── terrain.dds           sol, répété en damier
```

Le client cherche du **plus précis au plus général** : `player_weapon_3`, puis
`player_weapon`, puis `player`, puis une forme géométrique. Aucun de ces
fichiers n'est obligatoire.

Extensions reconnues, dans l'ordre : `dds`, `tga`, `png`, `webp`, `jpg`.

## Ce qui marche directement, et ce qui demande une conversion

| Format d'origine | Contenu | État |
|---|---|---|
| `.dds` | textures | **lu directement** par Godot |
| `.tga` | textures avec alpha | **lu directement** |
| `.epk` / `.eix` | archives | à extraire d'abord (Eternexus) |
| `.gr2` | modèles et animations | **non lisible** — voir ci-dessous |
| `.msa` / `.msm` | effets, auras | **non lisible**, format propriétaire |
| cartes | terrain, villages | **non lisible**, format propriétaire |

### Les modèles `.gr2`

Granny 3D est un format propriétaire fermé (RAD Game Tools). Godot ne le lit
pas, et il n'existe pas de convertisseur direct vers glTF. La chaîne est :

```
.gr2  →  .fbx  →  Blender  →  .glb
```

Les outils existent, mais **tous exigent Windows** et une `granny2.dll`
provenant d'une installation licenciée :

- [GrannyConverterLibrary](https://github.com/Anohros/GrannyConverterLibrary) —
  `granny2.dll` 2.9 à 2.12, Windows uniquement
- [GR2 → FBX Exporter](https://metin2.dev/topic/28691-gr2-fbx-exporter-source/)
  sur Metin2Dev, avec les sources
- Noesis et son greffon Granny2, pour des extractions rapides

Défauts de conversion documentés : os et animations qui se décalent (une épaule
droite qui atterrit à gauche), perte des shaders, des propriétés de matériaux et
des poids de squelette. Les convertisseurs en ligne génériques échouent sur ce
format.

**Et même convertis, ces modèles ne serviraient pas au client actuel** : il est
en 2D vue de dessus. Les afficher demanderait un client 3D — un autre chantier,
avec caméra, animations et éclairage.

## Ce qui est réaliste aujourd'hui

Les **textures** fonctionnent tout de suite, sans rien convertir. C'est ce qui
change le plus l'aspect pour le moins d'effort : poser `player.dds`,
`creature.dds` et `terrain.dds` suffit à remplacer les disques par des sprites
et à texturer le sol.

Le reste — modèles, animations, cartes, effets — demande une chaîne d'outils
Windows, et un client 3D qui n'existe pas.

## Si le projet doit un jour distribuer des assets

Il lui en faudra dont il a les droits. Committer des fichiers tiers ferait de ce
dépôt public une redistribution, et l'historique git les conserverait après
suppression. Trois voies dans [la roadmap](../../docs/ROADMAP.md) : CC0, packs
achetés, ou production maison. Ces assets-là iraient dans un dossier versionné,
distinct de celui-ci, avec leurs licences.
