# Client Hwarang

Vue de dessus en 2D, en Godot 4.7. Le serveur raisonne en centimètres sur un
plan : une vue de dessus les représente sans rien inventer, là où une scène 3D
demanderait des modèles et des animations qui n'existent pas encore.

## Lancer

Le serveur doit tourner (`cargo run -p hwarang-server`).

```bash
# Depuis la racine du dépôt
godot --path client -- --port 13000 --account moi

# Deux clients côte à côte, pour se voir bouger
godot --path client -- --account alice &
godot --path client -- --account bob &
```

Le compte est créé s'il n'existe pas ; sinon le client s'y connecte.

| Touche | Effet |
|---|---|
| ZQSD / flèches | se déplacer |
| espace | attaquer la cible la plus proche, ou se relever |

## Test d'intégration

```bash
godot --headless --path client --script res://tests/integration.gd -- --port 13000
```

Il vérifie ce qu'aucun test unitaire ne peut atteindre : que l'encodage
gros-boutiste de `StreamPeerBuffer` correspond à ce que le serveur Rust émet.
Une inversion d'octets passerait toutes les vérifications internes de chaque
côté et ne se verrait qu'ici. La CI l'exécute à chaque poussée.

## Ce que le client fait, et ne fait pas

**Prédiction locale.** Le déplacement s'affiche immédiatement, sans attendre le
serveur — sinon chaque pas coûte un aller-retour et le personnage colle aux
doigts. Le serveur confirme en silence ; quand il refuse, `MoveRejected` remet le
joueur où il est réellement.

**Ce traitement n'est pas optionnel.** Un client qui ignore `MoveRejected` reste
en avance : il calcule le pas suivant depuis une position imaginaire, ce pas est
refusé à son tour, et l'écart s'aggrave jusqu'à immobiliser le personnage — sans
aucune erreur affichée, alors que le serveur se comporte correctement.

**Interpolation.** Le serveur envoie une position toutes les 200 ms ; s'y tenir
donnerait un mouvement à cinq images par seconde. L'interpolation comble
l'intervalle à l'affichage sans rien changer à ce que le serveur sait.

**Aucune règle de jeu.** Le client n'estime pas les dégâts, ne décide pas ce qui
est à portée pour de bon, n'anticipe pas une mort. Il affiche ce que le serveur
lui dit.

## Assets

`assets/` n'est pas versionné (voir [son README](assets/README.md)). Le client
fonctionne sans : il affiche alors des disques — bleus pour les joueurs, rouges
pour les créatures, cerclés de blanc pour soi-même.

S'il trouve `assets/textures/player.png` ou `creature.png`, il les utilise à la
place. Les modèles 3D d'origine de Metin2 (`.gr2`, format Granny propriétaire)
ne sont pas lisibles par Godot et demanderaient une conversion.
