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

### J4 — Persistance
Comptes avec mots de passe hachés (Argon2), personnages sauvegardés en SQLite.
Le nom de compte est insensible à la casse et restreint à l'ASCII : accepter
l'Unicode entier ouvrirait l'usurpation par homographie. Un compte inexistant et
un mot de passe faux donnent la même erreur, après le même temps de calcul —
sinon l'écart suffit à énumérer les comptes du serveur.
**Démo :** `scripts/persistence.py`, en deux phases autour d'un redémarrage.

### J5 — Monde peuplé
Créatures autonomes et boucle de simulation à pas fixe. Jusqu'ici tout était
piloté par les messages entrants ; une créature agit sans que personne ne parle.

La règle d'agressivité vit dans le domaine et ne décide que d'une **intention** :
elle ne manipule aucun identifiant et ne déplace rien. La laisse se mesure depuis
le **poste** de la créature, jamais depuis sa cible — sinon un joueur traîne un
troupeau à travers la carte pour le déposer sur quelqu'un d'autre.

Les postes sont espacés d'au moins deux fois le rayon d'agressivité : en
approcher un ne doit jamais en réveiller deux. Sans cette contrainte, un
personnage neuf se fait submerger sans avoir rien fait de maladroit — constaté
en jouant la démonstration, pas en relisant le code.
**Démo :** `scripts/creatures.py`

### J6 — Objets
Catalogue, sac à emplacements stables, équipement, butin. Les définitions sont
une **donnée** passée au monde : rééquilibrer une arme ne demande pas de
recompiler, et les bonus étant relus au calcul plutôt que recopiés à
l'équipement, le rééquilibrage s'applique aux objets déjà portés.

Le sac ne se tasse pas quand on en retire un objet : un client qui affiche une
grille verrait sinon son contenu se réorganiser sous le curseur.

**Régénération hors combat**, ajoutée ici parce que la démonstration l'a rendue
inévitable : sans elle, les dégâts s'accumulent d'un combat au suivant et le seul
moyen de repartir en pleine santé est de mourir. Constaté en enchaînant deux
adversaires, pas en relisant le code.
**Démo :** `scripts/items.py`, en deux phases autour d'un redémarrage.

### J3 — Client Godot
Vue de dessus 2D, Godot 4.7 (arm64 natif, vérifié). Prédiction locale du
déplacement, interpolation entre les positions reçues, réconciliation sur
`MoveRejected`.

**L'inconnue est levée** : lire du binaire gros-boutiste depuis GDScript
fonctionne, via `StreamPeerBuffer.big_endian`. Ce n'est pas le défaut — sans
cette ligne, chaque entier arrive à l'envers et le flux paraît corrompu dès la
première trame.

Un défaut d'interopérabilité trouvé au passage : les identifiants de créatures
descendaient de `u64::MAX`, et GDScript n'a que des entiers **signés**.
`u64::MAX` s'y affichait `-1`. L'aller-retour restait juste — mêmes bits — mais
les journaux devenaient illisibles. Les identifiants partent désormais de `2^62`,
sous `i64::MAX`.

**Démo :** `client/tests/integration.gd`, exécuté par la CI contre le vrai
serveur.

---

## Phase 1 — Faire du serveur un jeu

Technique pure, solo, effort prévisible. C'est la partie où le projet avance
vite, et il faut en profiter pour la finir proprement.

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

**Distinction à ne pas perdre de vue.** Ce qu'on utilise localement et ce que le
dépôt distribue sont deux choses différentes. `client/assets/` est exclu du
versionnement : chacun y met ce qu'il veut sur sa machine, et le client
fonctionne sans — il affiche alors des formes géométriques.

Le jour où le projet doit *distribuer* des assets, il lui en faudra dont il a les
droits. Committer des fichiers Ymir dans un dépôt public serait de la
redistribution, avec DMCA sur le dépôt et strike sur le compte ; et l'historique
git les conserverait après suppression, obligeant à le réécrire en entier.

### L'autre mur : la durée

Un MMO solo n'échoue pas sur un obstacle technique, il s'arrête par lassitude
pendant la phase de contenu — celle qui n'a pas de fin et où chaque heure
produit un résultat plus petit que la précédente.

D'où l'ordre choisi ici : le serveur est désormais un jeu jouable, même sans
image. Tout ce qui vient après ajoute de la matière, pas des fondations.

---

## Décisions ouvertes

| Sujet | État |
|---|---|
| Dépôt public ou privé | **public** depuis le 08/08/2026 ; la CI Actions fonctionne (gratuite et illimitée en public) |
| Origine des assets | non tranchée. `client/assets/` est hors versionnement : ce qu'on y met localement ne regarde que soi. Pour *distribuer*, il faudra des assets dont le projet a les droits |
| Base de données | SQLite en place ; PostgreSQL si la charge le justifie, le domaine n'en dépend pas |
| Chiffrement du transport | absent ; à traiter avant toute exposition hors réseau local |

## Audit — ce qui a été trouvé et corrigé

Trois relectures adversariales (serveur, domaine, protocole/sécurité) ont
produit trois défauts réels, tous corrigés :

**Déni de service par file de sortie non bornée.** Un client pouvait entrer dans
le monde puis cesser de lire sa socket. Son écriture réseau restait en attente,
mais les autres joueurs continuaient d'alimenter sa file, laquelle croissait
sans limite jusqu'à épuiser la mémoire du serveur — pour tout le monde, à partir
d'un seul client. Le canal est désormais borné (`OUTBOX_CAPACITY`), avec un test
qui pousse 10 000 événements vers un client inactif et vérifie le plafond.

*Limite assumée* : un message écrêté peut être un `EntityVanished`, ce qui laisse
un fantôme à l'écran du client en retard. Distinguer les messages perdables
(`EntityMoved`, remplacé par le suivant) de ceux qui portent une transition
unique demandera de fermer la session plutôt que d'écrêter — à traiter quand le
client existera et qu'on pourra observer le comportement.

**Expérience résiduelle au palier terminal.** La boucle de franchissement
sortait sans consommer le seuil en atteignant le niveau maximum, laissant une
expérience très supérieure au seuil de son propre palier. Une barre de
progression calculée comme `expérience / seuil` aurait dépassé 100 %. Corrigé,
avec un test qui verrouille l'invariant « l'expérience est un reliquat, jamais
un cumul » sur toute la plage de gains.

**Documentation mensongère sur la cadence.** `request_attack` annonçait mesurer
le temps depuis la dernière attaque *retenue* ; le code mesure depuis la
dernière *tentative*, refusée comprise. Le comportement est le bon — compter les
seules attaques abouties laisserait accumuler du temps à coups de refus puis le
dépenser en salve — mais un mainteneur qui aurait fait confiance au commentaire
aurait introduit la faille en « corrigeant » le code.

Un bug de régression a été introduit puis attrapé pendant la correction du DoS :
sur un canal borné `Sender::send` est asynchrone, et le `let _ =` qui traînait
masquait un futur jamais attendu — plus aucun `EntityVanished` n'était émis à la
déconnexion. Seul un test existant l'a vu.

## Anomalie résolue — dérive du client après un refus

Les démonstrations enchaînées échouaient environ une fois sur cinq. **Trois
hypothèses ont été réfutées par la mesure avant de trouver la vraie cause**, et
elles méritent d'être notées parce qu'elles étaient toutes plausibles :

| Hypothèse | Réfutée par |
|---|---|
| Fuite d'entités entre sessions | le journal serveur affiche `0 en jeu` après chaque passe |
| Reliquat d'état entre scénarios | `combat` puis `two_clients`, cinq fois : aucun échec |
| Instabilité aléatoire | l'échec est déterministe une fois la bonne variable isolée |

**Cause réelle**, dans le script de démonstration et non dans le serveur : après
un déplacement refusé, le client conservait la position qu'il avait *demandée*
au lieu de celle que le serveur lui *réaffirmait*. Il calculait alors le pas
suivant depuis un point imaginaire, ce pas était refusé à son tour, et l'écart
s'aggravait jusqu'à immobiliser le personnage.

L'intermittence venait de l'amplitude de l'écart : la ligne de la grille
d'apparition dépend de l'identifiant d'entité, donc du nombre de connexions déjà
servies. Un écart de 300 cm était absorbé par la tolérance du premier pas ; un
écart de 1200 cm ne l'était pas. En forçant les identifiants, l'échec devient
reproductible à 100 %.

**Correctif :** les clients de démonstration appliquent `MoveRejected` à leur
état local — ce que `MoveRejected` sert précisément à permettre.

**À retenir pour le client Godot (J3) :** traiter cette trame n'est pas
optionnel. Un client qui l'ignore présente un personnage qui « ne répond plus »,
sans aucune erreur affichée, alors que le serveur se comporte correctement.
