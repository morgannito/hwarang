# Hwarang

MMORPG action orienté combat, dans la lignée de Metin2 — réécrit intégralement,
sans une ligne de code ni un octet d'asset repris au jeu d'origine.

> Les *hwarang* étaient un corps de jeunes guerriers du royaume de Silla
> (Corée, VI<sup>e</sup> siècle). Nom historique, donc libre de droits.

## Pourquoi repartir de zéro

L'écosystème des serveurs privés Metin2 repose sur des binaires et des sources
fuités de Ymir/GameForge. Cela impose trois plafonds qu'aucune quantité de
travail ne fait sauter :

| Plafond | Conséquence |
|---|---|
| Code propriétaire | Projet indiffusable, non ouvrable, DMCA à la moindre visibilité |
| Direct3D 8 + Granny 3D (closed-source) | Pas de rendu moderne, pas de portage hors Windows |
| C++ 2005, x86 32 bits | Pas d'Apple Silicon, pas de Linux, pas de mobile |

Réécrire coûte plus cher au départ et supprime les trois d'un coup.

## Ce qui change par rapport à l'original

**Mitigation bornée par construction.** Metin2 empile armure et résistances de
façon additive : au-delà d'un certain équipement, les dégâts tombent à zéro et
le personnage devient intuable. Chaque serveur privé rustine le symptôme.
Ici les deux réductions sont multiplicatives et le plafond de résistance est
porté par le type `Resistance` — aucun jeu de statistiques ne peut produire
une invincibilité. Voir [ADR-0003](docs/adr/0003-mitigation-multiplicative-bornee.md).

**Courbe de progression paramétrable.** L'original embarque 120 constantes
compilées dans le binaire ; rejouer l'équilibrage impose de recompiler le
serveur. `ProgressionCurve` est une donnée du domaine.

**Franchissement de paliers multiples en une passe.** Un gain massif d'expérience
n'est ni tronqué à un palier, ni renvoyé à l'appelant pour rejouer la règle.

**Serveur autoritaire sur les déplacements.** Metin2 laisse le client annoncer
sa position et le croit sur parole — c'est l'origine du speedhack endémique de
l'écosystème. Ici le client *propose*, le serveur vérifie que la distance est
compatible avec le temps écoulé (mesuré côté serveur), et en cas de refus
renvoie sa propre position pour resynchroniser sans aller-retour.

**Domaine sans I/O.** `hwarang-domain` n'a aucune dépendance : chaque règle de
jeu est testable sans lancer un serveur ni une base de données.

## Structure

```
crates/
├── domain/     Règles de jeu. Zéro dépendance, zéro I/O.
│   ├── character/  progression, attributs
│   ├── combat/     résolution d'attaque
│   └── world/      positions, mouvement, grille d'intérêt
├── protocol/   Trames binaires client/serveur. Encodage explicite.
└── server/     Adaptateur TCP + registre d'entités. Aucune règle de jeu.
```

La dépendance ne va que dans un sens : `server → protocol → domain`.

## Démarrer

```bash
cargo test --workspace          # 82 tests
cargo run -p hwarang-server     # écoute sur 127.0.0.1:13000
HWARANG_BIND=0.0.0.0:13000 cargo run -p hwarang-server
```

Deux vérifications bout en bout, contre un serveur en cours d'exécution. Les
deux réimplémentent le format binaire au lieu de réutiliser `hwarang-protocol` :
un test qui partagerait l'encodage du serveur ne prouverait rien sur ce qui
circule réellement.

```bash
python3 scripts/smoke.py 127.0.0.1 13000         # protocole, cas hostiles
python3 scripts/two_clients.py 127.0.0.1 13000   # diffusion entre deux joueurs
```

## État

| Composant | État |
|---|---|
| Domaine : progression, jauges, combat | testé |
| Domaine : positions, mouvement, grille d'intérêt | testé |
| Protocole binaire + machine à états de session | testé |
| Serveur TCP, registre d'entités, diffusion aux voisins | fonctionnel |
| Persistance | à faire |
| Client (Godot 4) | à faire |

Ce qui marche aujourd'hui : deux clients se connectent, se découvrent, se voient
bouger en temps réel, disparaissent l'un de l'autre en sortant du champ de
vision, et un déplacement impossible est refusé puis corrigé par le serveur.

## Décisions d'architecture

- [ADR-0001 — Réécriture intégrale, aucun code propriétaire](docs/adr/0001-reecriture-integrale.md)
- [ADR-0002 — Rust côté serveur, Godot 4 côté client](docs/adr/0002-stack-rust-godot.md)
- [ADR-0003 — Mitigation multiplicative bornée](docs/adr/0003-mitigation-multiplicative-bornee.md)

## Licence

AGPL-3.0-only. Un serveur de jeu est un service réseau : la clause AGPL est ce
qui garantit que les améliorations apportées par un hébergeur restent
accessibles aux joueurs.
