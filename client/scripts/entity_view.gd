## Representation d'une entite a l'ecran.
##
## Interpole entre les positions recues. Le serveur envoie un point tous les
## 200 ms ; s'y tenir donnerait un deplacement par saccades de cinq images par
## seconde. L'interpolation ne change rien a ce que le serveur sait, elle comble
## l'intervalle a l'affichage.
class_name EntityView
extends Node2D

## Duree pour rejoindre la derniere position connue.
##
## Calee sur le pas de simulation : plus court produit des a-coups, plus long
## fait trainer l'entite derriere sa position reelle.
const SMOOTHING := 0.2

## Un centimetre de monde vaut ce nombre de pixels.
const SCALE := 0.05

## Taille visee pour un sprite, en pixels.
##
## Les textures d'origine varient de 32 a 512 pixels : les ramener a une taille
## commune evite qu'un asset plus grand n'ecrase visuellement les autres.
const SPRITE_TARGET_PX := 28.0

var entity_id: int = 0
var is_player := false
var is_self := false
var health_ratio := 1.0
var down := false
## Objet equipe en arme, 0 si aucun. Sert a choisir la texture.
var weapon_item := 0

var _target := Vector2.ZERO
var _sprite: Node2D
var _label: Label


func setup(id: int, kind: int, world_position: Vector2, own: bool) -> void:
	entity_id = id
	is_player = kind == Protocol.KIND_PLAYER
	is_self = own
	_target = world_position * SCALE
	position = _target
	_build()


## Nouvelle position connue : l'affichage la rejoint progressivement.
func move_to(world_position: Vector2) -> void:
	_target = world_position * SCALE


## Position sans interpolation, pour une apparition ou une reapparition.
func teleport_to(world_position: Vector2) -> void:
	_target = world_position * SCALE
	position = _target


func _process(delta: float) -> void:
	position = position.lerp(_target, minf(delta / SMOOTHING, 1.0))
	queue_redraw()


func _build() -> void:
	_label = Label.new()
	_label.position = Vector2(-24, -34)
	_label.add_theme_font_size_override("font_size", 10)
	add_child(_label)

	_refresh_sprite()


## (Re)choisit la texture, du plus precis au plus general.
##
## L'equipement affiche en priorite : changer d'arme doit se voir. Sans asset
## correspondant, on retombe sur la silhouette du type, puis sur rien du tout —
## et `_draw` dessine alors une forme geometrique.
func _refresh_sprite() -> void:
	var base := "player" if is_player else "creature"
	var candidates: Array[String] = []
	if weapon_item > 0:
		candidates.append("%s_weapon_%d" % [base, weapon_item])
		candidates.append("%s_weapon" % base)
	candidates.append(base)

	var texture := AssetLibrary.first("textures", candidates)
	if texture == null:
		if _sprite != null:
			_sprite.queue_free()
			_sprite = null
		return

	if _sprite == null:
		var sprite := Sprite2D.new()
		add_child(sprite)
		_sprite = sprite
	(_sprite as Sprite2D).texture = texture
	# Les textures d'origine sont souvent bien plus grandes que l'echelle de la
	# vue : on les ramene a une taille lisible plutot que d'imposer un format.
	var size := texture.get_size()
	var largest := maxf(size.x, size.y)
	_sprite.scale = Vector2.ONE * (SPRITE_TARGET_PX / maxf(largest, 1.0))


## Change l'arme portee et met a jour l'apparence.
func set_weapon(item: int) -> void:
	if weapon_item == item:
		return
	weapon_item = item
	_refresh_sprite()


func _draw() -> void:
	_label.text = "%s%s" % ["moi" if is_self else "", " (a terre)" if down else ""]

	if _sprite != null:
		_draw_health_bar()
		return

	# Sans asset : un disque, plus un liseré pour se distinguer soi-meme.
	var color := Color(0.35, 0.65, 1.0) if is_player else Color(0.85, 0.4, 0.3)
	if down:
		color = color.darkened(0.6)
	draw_circle(Vector2.ZERO, 9.0, color)
	if is_self:
		draw_arc(Vector2.ZERO, 12.0, 0, TAU, 24, Color.WHITE, 2.0)
	_draw_health_bar()


func _draw_health_bar() -> void:
	if health_ratio >= 1.0 and not down:
		return
	var width := 26.0
	draw_rect(Rect2(-width / 2, -22, width, 4), Color(0.15, 0.15, 0.15))
	draw_rect(
		Rect2(-width / 2, -22, width * clampf(health_ratio, 0.0, 1.0), 4),
		Color(0.3, 0.85, 0.4) if health_ratio > 0.3 else Color(0.9, 0.3, 0.3)
	)
