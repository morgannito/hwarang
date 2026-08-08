## Scene principale : relie la connexion a l'affichage.
##
## Le client **propose** un deplacement et l'affiche aussitot, sans attendre la
## reponse : sinon chaque pas coute un aller-retour reseau et le personnage
## semble coller aux doigts. Quand le serveur refuse, `move_rejected` remet le
## joueur ou il est reellement — traiter cette trame n'est pas optionnel, un
## client qui l'ignore derive jusqu'a l'immobilite.
extends Node2D

## Vitesse de course du serveur, en centimetres par seconde.
const RUN_SPEED := 700.0

## Intervalle entre deux annonces de position.
##
## Le serveur borne la distance par pas ; emettre plus souvent ne fait pas
## avancer plus vite et ne ferait qu'ajouter du trafic.
const MOVE_INTERVAL := 0.2

## Portee du corps a corps, cote serveur.
const MELEE_RANGE := 200.0

@onready var connection: Connection = $Connection
@onready var entities: Node2D = $Entities
@onready var camera: Camera2D = $Camera2D
@onready var status: Label = $UI/Status
@onready var log_label: Label = $UI/Log

var _views := {}
var _position := Vector2.ZERO
var _since_last_move := 0.0
var _lines: Array[String] = []
var _down := false


func _ready() -> void:
	connection.world_entered.connect(_on_world_entered)
	connection.entity_appeared.connect(_on_entity_appeared)
	connection.entity_moved.connect(_on_entity_moved)
	connection.entity_vanished.connect(_on_entity_vanished)
	connection.move_rejected.connect(_on_move_rejected)
	connection.damage_dealt.connect(_on_damage_dealt)
	connection.entity_died.connect(_on_entity_died)
	connection.entity_respawned.connect(_on_entity_respawned)
	connection.experience_gained.connect(_on_experience_gained)
	connection.item_received.connect(_on_item_received)
	connection.equipment_changed.connect(_on_equipment_changed)
	connection.notice.connect(_log)
	connection.disconnected.connect(func(reason): _log("deconnecte : " + reason))

	_draw_ground()

	var host := _argument("--host", "127.0.0.1")
	var port := int(_argument("--port", "13000"))
	var account := _argument("--account", "godot-%d" % (Time.get_ticks_msec() % 100000))
	_log("connexion a %s:%d" % [host, port])
	connection.connect_to_server(host, port, account, "mot-de-passe-jetable")


## Lit un argument de ligne de commande, pour lancer deux clients cote a cote.
func _argument(name: String, fallback: String) -> String:
	var arguments := OS.get_cmdline_user_args()
	var index := arguments.find(name)
	if index >= 0 and index + 1 < arguments.size():
		return arguments[index + 1]
	return fallback


func _process(delta: float) -> void:
	_update_status()
	if connection.state != Connection.State.IN_WORLD or _down:
		return

	var direction := Input.get_vector("move_left", "move_right", "move_up", "move_down")
	if direction == Vector2.ZERO:
		return

	# Prediction locale : l'affichage suit la touche, le serveur confirme ou
	# corrige ensuite.
	_position += direction.normalized() * RUN_SPEED * delta
	if _views.has(connection.entity_id):
		_views[connection.entity_id].move_to(_position)
	camera.position = _position * EntityView.SCALE

	_since_last_move += delta
	if _since_last_move >= MOVE_INTERVAL:
		_since_last_move = 0.0
		connection.request_move(_position)


func _unhandled_input(event: InputEvent) -> void:
	if not event.is_action_pressed("attack"):
		return
	if _down:
		connection.request_respawn()
		return
	var target := _nearest_target()
	if target > 0:
		connection.request_attack(target)
	else:
		_log("aucune cible a portee")


## Entite hostile la plus proche, dans l'allonge du corps a corps.
func _nearest_target() -> int:
	var best := 0
	var best_distance := MELEE_RANGE
	for id in _views:
		var view: EntityView = _views[id]
		if id == connection.entity_id or view.down:
			continue
		var distance := _position.distance_to(view._target / EntityView.SCALE)
		if distance <= best_distance:
			best_distance = distance
			best = id
	return best


func _on_world_entered(id: int, world_position: Vector2) -> void:
	_position = world_position
	_spawn(id, Protocol.KIND_PLAYER, world_position, true)
	camera.position = world_position * EntityView.SCALE
	_log("entre dans le monde en (%d, %d)" % [world_position.x, world_position.y])


func _on_entity_appeared(id: int, kind: int, world_position: Vector2) -> void:
	_spawn(id, kind, world_position, false)


func _spawn(id: int, kind: int, world_position: Vector2, own: bool) -> void:
	if _views.has(id):
		return
	var view := EntityView.new()
	view.set_script(load("res://scripts/entity_view.gd"))
	entities.add_child(view)
	view.setup(id, kind, world_position, own)
	_views[id] = view


func _on_entity_moved(id: int, world_position: Vector2) -> void:
	if _views.has(id):
		_views[id].move_to(world_position)


func _on_entity_vanished(id: int) -> void:
	if _views.has(id):
		_views[id].queue_free()
		_views.erase(id)


func _on_move_rejected(world_position: Vector2) -> void:
	# Realignement sur la verite du serveur. Sans lui, le pas suivant part d'un
	# point imaginaire, est refuse a son tour, et l'ecart ne se resorbe jamais.
	_position = world_position
	if _views.has(connection.entity_id):
		_views[connection.entity_id].teleport_to(world_position)
	camera.position = world_position * EntityView.SCALE


func _on_damage_dealt(attacker: int, target: int, damage: int, remaining: int) -> void:
	if _views.has(target):
		var view: EntityView = _views[target]
		# Les points maximum ne sont pas transmis : le ratio se deduit du plus
		# haut total observe, ce qui suffit a une barre indicative.
		view.set_meta("max_health", maxi(view.get_meta("max_health", remaining), remaining))
		var maximum: int = maxi(view.get_meta("max_health", 1), 1)
		view.health_ratio = float(remaining) / float(maximum)
	if target == connection.entity_id:
		_log("touche : -%d PV, il reste %d" % [damage, remaining])
	elif attacker == connection.entity_id:
		_log("coup porte : -%d PV, reste %d" % [damage, remaining])


func _on_entity_died(id: int, _killer: int) -> void:
	if _views.has(id):
		_views[id].down = true
		_views[id].health_ratio = 0.0
	if id == connection.entity_id:
		_down = true
		_log("vous etes a terre — espace pour revenir")


func _on_entity_respawned(id: int, world_position: Vector2, _health: int) -> void:
	if _views.has(id):
		var view: EntityView = _views[id]
		view.down = false
		view.health_ratio = 1.0
		view.teleport_to(world_position)
	if id == connection.entity_id:
		_down = false
		_position = world_position
		camera.position = world_position * EntityView.SCALE
		_log("de retour en jeu")


func _on_experience_gained(amount: int, level: int) -> void:
	_log("+%d XP (palier %d)" % [amount, level])


func _on_item_received(item: int, slot_index: int) -> void:
	_log("butin : objet %d en case %d — [%d] pour l'equiper" % [item, slot_index, slot_index])


func _on_equipment_changed(slot: int, item: int) -> void:
	var place := "arme" if slot == 1 else "armure"
	_log("%s : %s" % [place, "retiree" if item == 0 else "objet %d" % item])
	# L'arme change la silhouette si un asset correspondant existe.
	if slot == 1 and _views.has(connection.entity_id):
		_views[connection.entity_id].set_weapon(item)


func _update_status() -> void:
	var state_text := ["hors ligne", "poignee de main", "authentification", "pret", "en jeu"]
	status.text = "%s — (%d, %d) — %d entites visibles" % [
		state_text[connection.state], _position.x, _position.y, _views.size()
	]


## Pose le sol : une texture repetee si elle existe, une grille sinon.
func _draw_ground() -> void:
	var texture := AssetLibrary.texture("ground", "terrain")
	if texture == null:
		return
	var tiles := TextureRect.new()
	tiles.texture = texture
	tiles.stretch_mode = TextureRect.STRETCH_TILE
	# Assez large pour couvrir la zone de depart et ses environs.
	tiles.size = Vector2(4000, 4000)
	tiles.position = -tiles.size / 2
	tiles.z_index = -100
	add_child(tiles)
	move_child(tiles, 0)


func _log(line: String) -> void:
	_lines.append(line)
	if _lines.size() > 12:
		_lines.pop_front()
	log_label.text = "\n".join(_lines)
	print(line)
