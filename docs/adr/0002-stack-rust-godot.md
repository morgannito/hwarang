# ADR-0002 — Rust côté serveur, Godot 4 côté client

- **Statut** : accepté
- **Date** : 2026-08-08

## Contexte

[ADR-0001](0001-reecriture-integrale.md) impose de tout réécrire. Le choix de
stack n'est donc contraint par aucun héritage.

Besoins serveur : connexions longues et nombreuses, protocole binaire,
simulation à pas fixe, correction mémoire non négociable (un serveur de jeu est
exposé en permanence à des entrées hostiles).

Besoins client : rendu 3D, éditeur de scènes et d'animations, export
multi-plateforme, développement possible sur Apple Silicon.

## Décision

**Serveur : Rust + Tokio.** Absence de classe entière de vulnérabilités mémoire
sur du code exposé au réseau ; `unsafe_code = "forbid"` au niveau du workspace ;
types somme pour modéliser les machines à états de protocole avec exhaustivité
vérifiée à la compilation ; pas de pauses GC dans une boucle de simulation.

**Client : Godot 4.** Gratuit, sans redevance, sans clause d'usage. Éditeur natif
Apple Silicon, donc développable sur le poste de travail courant. Export
Windows / Linux / macOS depuis n'importe quelle plateforme. Le rendu passe par
Vulkan et Metal — précisément ce que Direct3D 8 et Granny interdisaient.

**Protocole : encodage binaire explicite**, sans bibliothèque de sérialisation
dérivée. Le format sur le fil est un contrat versionné entre deux binaires
déployés séparément ; il ne doit pas se déplacer parce qu'un champ Rust a été
renommé.

## Alternatives écartées

**Node.js / TypeScript** — itération plus rapide, mais coût mémoire par
connexion et pauses GC dans la boucle de simulation. Le projet de référence
[open-mt2](https://github.com/willianmarquess/open-mt2) valide l'approche pour
un prototype ; pas retenue pour la cible.

**Bevy (client Rust)** — un seul langage sur toute la pile, mais pas d'éditeur
visuel : le contenu deviendrait le goulot d'étranglement, alors que c'est déjà
le poste le plus cher (ADR-0001).

**Unity** — historique de changements de licence unilatéraux. Écarté pour un
projet destiné à durer et à rester ouvert.

## Conséquences

Compilation plus lente et courbe d'apprentissage plus raide qu'un langage
dynamique. Deux langages à maintenir (Rust, GDScript) avec un protocole binaire
comme frontière — d'où l'exigence de tests d'aller-retour exhaustifs sur
`hwarang-protocol`.
