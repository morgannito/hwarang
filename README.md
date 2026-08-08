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

**Autorité serveur sur l'offensive aussi.** Portée et cadence sont vérifiées
côté serveur, avec le temps mesuré par le serveur. Un client modifié qui envoie
mille attaques par seconde en voit passer une.

**Domaine sans I/O.** `hwarang-domain` n'a aucune dépendance : chaque règle de
jeu est testable sans lancer un serveur ni une base de données.

## Structure

```
crates/
├── domain/     Règles de jeu. Zéro dépendance, zéro I/O.
│   ├── ai/         perception, agressivité, laisse
│   ├── character/  progression, attributs, régénération
│   ├── item/       catalogue, inventaire, équipement
│   ├── combat/     dégâts, engagement (portée, cadence)
│   └── world/      positions, mouvement, grille d'intérêt
├── protocol/   Trames binaires client/serveur. Encodage explicite.
├── storage/    Comptes et sauvegardes. SQLite + Argon2.
└── server/     Adaptateur TCP + registre d'entités. Aucune règle de jeu.

client/         Client Godot 4 (vue de dessus 2D). Aucune règle de jeu.
```

La dépendance ne va que dans un sens : `server → {protocol, storage} → domain`.

## Démarrer

```bash
cargo test --workspace          # 244 tests
cargo run -p hwarang-server     # 127.0.0.1:13000, base ./hwarang.sqlite
HWARANG_BIND=0.0.0.0:13000 HWARANG_DB=/var/lib/hwarang.sqlite \
  cargo run -p hwarang-server
```

Six vérifications bout en bout, contre un serveur en cours d'exécution. Elles
réimplémentent le format binaire au lieu de réutiliser `hwarang-protocol` : un
test qui partagerait l'encodage du serveur ne prouverait rien sur ce qui circule
réellement.

```bash
python3 scripts/smoke.py 127.0.0.1 13000         # protocole, cas hostiles
python3 scripts/two_clients.py 127.0.0.1 13000   # diffusion entre deux joueurs
python3 scripts/combat.py 127.0.0.1 13000        # portée, cadence, mort, XP
python3 scripts/creatures.py 127.0.0.1 13000     # créatures autonomes
python3 scripts/persistence.py 127.0.0.1 13000 phase1   # puis redémarrer,
python3 scripts/persistence.py 127.0.0.1 13000 phase2   # même HWARANG_DB
python3 scripts/items.py 127.0.0.1 13000 phase1         # butin, équipement,
python3 scripts/items.py 127.0.0.1 13000 phase2         # puis redémarrer
```

Chacune attend un monde et une base vierges. La CI démarre un serveur neuf par
démonstration : un scénario qui dépend de ce qu'a laissé le précédent ne dit
rien de clair quand il échoue.

## État

| Composant | État |
|---|---|
| Domaine : progression, jauges, mitigation, engagement | testé |
| Domaine : positions, mouvement, grille d'intérêt | testé |
| Protocole binaire + machine à états de session | testé |
| Serveur : registre d'entités, diffusion, combat | fonctionnel |
| Comptes (Argon2) et sauvegarde des personnages | fonctionnel |
| Créatures autonomes, boucle de simulation | fonctionnel |
| Butin, inventaire, équipement, régénération | fonctionnel |
| Client Godot : déplacement, combat, interpolation | fonctionnel |
| Art (modèles, animations, cartes) | inexistant — voir la roadmap |

Ce qui marche aujourd'hui : on crée un compte, on entre dans le monde, on voit
les autres joueurs bouger en temps réel, on les perd de vue en s'éloignant, on
s'affronte au corps à corps jusqu'à la mort de l'un — qui gagne de l'expérience
et peut réapparaître. **On se déconnecte, le serveur redémarre, et on retrouve
son personnage où on l'avait laissé.** Le monde est peuplé de créatures qui
remarquent le joueur, le poursuivent, ripostent sans qu'on leur parle, et
reviennent à leur poste après avoir été abattues.

Les créatures abattues laissent du butin ; l'équiper augmente les dégâts, et le
sac comme l'équipement survivent au redémarrage. Hors combat, les points de vie
reviennent — sans quoi la seule façon de repartir en pleine santé serait de
mourir.

Déplacement trop rapide, attaque hors de portée, rafale d'attaques, acharnement
sur un cadavre : refusés par le serveur, avec le motif.

## Client

```bash
godot --path client -- --port 13000 --account moi
```

ZQSD pour se déplacer, espace pour attaquer la cible la plus proche. Voir
[client/README.md](client/README.md).

Il affiche des disques : le projet n'a aucun art. `client/assets/` n'est pas
versionné — ce qu'on y pose localement ne regarde que soi, et le client
fonctionne sans.

## Suite

[Roadmap](docs/ROADMAP.md) — jalons, et les deux murs qui ne sont pas techniques.

## Décisions d'architecture

- [ADR-0001 — Réécriture intégrale, aucun code propriétaire](docs/adr/0001-reecriture-integrale.md)
- [ADR-0002 — Rust côté serveur, Godot 4 côté client](docs/adr/0002-stack-rust-godot.md)
- [ADR-0003 — Mitigation multiplicative bornée](docs/adr/0003-mitigation-multiplicative-bornee.md)

## Licence

AGPL-3.0-only. Un serveur de jeu est un service réseau : la clause AGPL est ce
qui garantit que les améliorations apportées par un hébergeur restent
accessibles aux joueurs.
