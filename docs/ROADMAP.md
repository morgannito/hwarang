# Roadmap

Chaque jalon se termine par une **démonstration exécutable**, pas par « le code
est écrit ». Si un jalon ne peut pas se montrer, il n'est pas fini.

Les jalons sont ordonnés pour qu'un arrêt du projet à n'importe quel moment
laisse quelque chose qui tient debout, plutôt qu'un chantier à moitié creusé.

Aucune date : c'est un projet du soir. Les tailles sont relatives entre elles.

---

## Fait

### J0 — Socle · `a91a1fd`
Domaine sans dépendance (progression, jauges, mitigation bornée), protocole
binaire à trames longueur-préfixée, serveur TCP, machine à états de session.
**Démo :** `scripts/smoke.py`

### J1 — Monde vivant · `359bcca`
Positions entières, validation de déplacement côté serveur, grille d'intérêt,
diffusion aux voisins, apparition/disparition au champ de vision.
**Démo :** `scripts/two_clients.py`

### J2 — Combat branché
`combat` et `character` étaient testés mais appelés par aucun chemin de code.
Portée, cadence, mort, expérience, réapparition — toutes vérifiées côté serveur.
**Démo :** `scripts/combat.py`

---

## Phase 1 — Faire du serveur un jeu

Technique pure, solo, effort prévisible. C'est la partie où le projet avance
vite, et il faut en profiter pour la finir proprement.

### J3 — Visualiseur Godot · **moyen**

Client minimal, **sans aucun art** : capsules, sol quadrillé, barres de vie.

- connexion `StreamPeerTCP`, décodage du protocole binaire en GDScript
- interpolation entre positions reçues, sinon le mouvement est saccadé
- réconciliation sur `MoveRejected`

**Fini quand :** deux fenêtres Godot côte à côte, on déplace l'une, l'autre le
voit ; un client bricolé pour tricher se fait replacer visiblement.

**Inconnue non levée :** lire du binaire big-endian depuis GDScript
(`PackedByteArray`, lectures partielles). Non vérifié à ce jour.

### J4 — Persistance · **moyen**

Aujourd'hui tout disparaît au redémarrage, et `session_id` est un compteur : il
n'y a **aucune authentification**.

- comptes avec mot de passe haché (Argon2)
- personnages sauvegardés : position, niveau, expérience, jauges
- SQLite pour commencer — un fichier, pas de serveur à administrer, migration
  vers PostgreSQL possible plus tard sans changer le domaine

**Fini quand :** on se connecte, on gagne un niveau, on redémarre le serveur, on
se reconnecte et le niveau est toujours là.

### J5 — Monde peuplé · **grand**

- entités non joueuses, machine à états d'IA (inactif → poursuite → combat →
  retour), table d'agression
- zones d'apparition, réapparition différée
- boucle de simulation à pas fixe : jusqu'ici tout est piloté par les messages
  entrants, un monstre doit agir sans que personne ne parle

**Fini quand :** un joueur peut affronter un monstre qui riposte, mourir, et
revenir.

### J6 — Objets · **grand**

Inventaire, équipement, statistiques dérivées de l'équipement, butin.
C'est le premier jalon dont le coût est autant en **données** qu'en code.

---

## Phase 2 — Là où ça devient réellement difficile

Rien de ce qui suit n'est un problème de programmation.

### Le mur : l'art

L'[ADR-0001](adr/0001-reecriture-integrale.md) interdit tout asset Ymir. Un MMO
demande des modèles, des animations par personnage et par sexe, des textures,
des effets, des cartes, de l'interface, du son. C'est le poste le plus cher du
projet, de très loin, et **aucune décision technique ne le réduit**.

Trois voies, à trancher avant J3 car elle oriente le style du client :

| Voie | Réalité |
|---|---|
| Assets libres (CC0 / Kenney, Quaternius, Mixamo…) | gratuit et immédiat, mais style hétérogène — le jeu ressemblera à un assemblage |
| Achat de packs cohérents | quelques centaines d'euros pour une direction artistique tenable ; vérifier que la licence couvre un usage en ligne |
| Production maison | seule voie vers une identité propre ; hors de portée en solo sans compétence 3D |

Décision non prise. La différence entre les trois n'est pas le budget, c'est
d'accepter ou non que le jeu n'ait pas d'identité visuelle.

### L'autre mur : la durée

Un MMO solo n'échoue pas sur un obstacle technique, il s'arrête par lassitude
pendant la phase de contenu — celle qui n'a pas de fin et où chaque heure
produit un résultat plus petit que la précédente.

D'où l'ordre choisi ici : à la fin de J3, le projet est déjà une démonstration
technique montrable. À la fin de J5, c'est un jeu jouable, même laid. Tout ce
qui vient après ajoute de la matière, pas des fondations.

---

## Décisions ouvertes

| Sujet | État |
|---|---|
| Dépôt public ou privé | privé ; la CI Actions est bloquée côté facturation, le passage en public la débloquerait (et l'ADR-0001 le permet) |
| Origine des assets | non tranchée — à décider avant J3 |
| Base de données | SQLite pour J4, PostgreSQL si la charge le justifie |
| Chiffrement du transport | absent ; à traiter avant toute exposition hors réseau local |

## Anomalies connues

**Démonstrations enchaînées sur un même serveur.** Lancer `smoke`, `two_clients`
et `combat` à la suite contre un seul serveur échoue environ une fois sur cinq.
Isolées, elles passent systématiquement (12/12 pour `two_clients` seule, 15/15
avec un serveur neuf par démonstration).

La cause n'est pas identifiée. L'hypothèse de travail est un reliquat d'état
entre scénarios — entités en cours de retrait, identifiants déjà distribués —
mais elle n'a pas été confirmée : les tentatives de reproduction ciblée
(`combat` puis `two_clients`, cinq fois) n'ont rien déclenché.

Contourné en CI par un serveur neuf par démonstration, ce qui est de toute façon
la bonne pratique. **Le contournement n'est pas un diagnostic** : si un
comportement dépend de l'état laissé par une session précédente, c'est une
propriété du serveur qu'il faudra comprendre avant d'ajouter la persistance.
