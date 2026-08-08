## Chargement des assets locaux.
##
## Cherche un fichier pour un nom logique, dans l'ordre des extensions que Godot
## sait lire. `DDS` et `TGA` viennent en premier : ce sont les formats dans
## lesquels arrivent la plupart des textures de jeux, et Godot les lit sans
## conversion.
##
## Tout est optionnel. Un nom sans fichier retourne `null`, et l'appelant
## retombe sur une forme geometrique — le client doit rester lancable sur une
## machine ou `assets/` est vide, sinon le projet ne se teste plus sans son art.
class_name AssetLibrary
extends RefCounted

## Ordre de recherche. `dds` et `tga` d'abord : Godot les importe directement,
## et ce sont les formats natifs de la plupart des jeux de cette generation.
const EXTENSIONS := ["dds", "tga", "png", "webp", "jpg"]

const ROOT := "res://assets"

## Cache des textures deja resolues, y compris les absences.
##
## Sans lui, chaque entite qui apparait relance une recherche sur le disque pour
## un fichier qui n'existe probablement pas.
static var _cache := {}


## Texture pour un nom logique, ou `null`.
##
## `dossier` correspond a un sous-dossier de `assets/` : `textures`, `ui`,
## `ground`…
static func texture(folder: String, name: String) -> Texture2D:
	var key := "%s/%s" % [folder, name]
	if _cache.has(key):
		return _cache[key]

	var found: Texture2D = null
	for extension in EXTENSIONS:
		var path := "%s/%s/%s.%s" % [ROOT, folder, name, extension]
		if ResourceLoader.exists(path):
			found = load(path) as Texture2D
			if found != null:
				break

	_cache[key] = found
	return found


## Premiere texture trouvee parmi plusieurs noms, du plus precis au plus general.
##
## Permet une texture par objet equipe, avec repli sur une silhouette generique :
## `player_weapon_3`, puis `player_weapon`, puis `player`.
static func first(folder: String, names: Array[String]) -> Texture2D:
	for name in names:
		var found := texture(folder, name)
		if found != null:
			return found
	return null


## Vrai si au moins un asset a ete trouve.
##
## Sert a signaler au lancement qu'on tourne en mode « formes geometriques »,
## plutot que de laisser croire a un probleme d'affichage.
static func any_loaded() -> bool:
	for key in _cache:
		if _cache[key] != null:
			return true
	return false
