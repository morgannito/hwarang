## Test d'integration : le client Godot contre le vrai serveur.
##
## Se lance sans fenetre :
##
##     Godot --headless --path client --script res://tests/integration.gd -- --port 13000
##
## Verifie ce qu'aucun test unitaire ne peut verifier : que l'encodage
## gros-boutiste de `StreamPeerBuffer` correspond bien a ce que le serveur Rust
## emet et attend. Une inversion d'octets passerait toutes les verifications
## internes du client et ne se verrait qu'ici.
extends SceneTree

const TIMEOUT_SECONDS := 25.0

var _connection: Connection
var _checks := {
	"authentifie": false,
	"entre dans le monde": false,
	"creature reperee": false,
	"deplacement legitime accepte": false,
	"saut refuse et position retablie": false,
}
## Le serveur ne confirme pas un deplacement accepte — seuls les voisins sont
## prevenus. Un pas legitime se constate donc par l'absence de refus, ce qui
## demande de compter les pas emis plutot que d'attendre une reponse.
var _legit_moves := 0
var _entered_at := Vector2.ZERO
var _started := 0.0
var _phase := "connexion"
var _steps := 0


func _initialize() -> void:
	_started = Time.get_ticks_msec() / 1000.0
	var port := int(_argument("--port", "13000"))
	print("test d'integration sur 127.0.0.1:%d" % port)

	_connection = Connection.new()
	root.add_child(_connection)

	_connection.authenticated.connect(func(_id): _pass("authentifie"))
	_connection.world_entered.connect(_on_world_entered)
	_connection.entity_appeared.connect(_on_entity_appeared)
	_connection.move_rejected.connect(_on_move_rejected)
	_connection.disconnected.connect(func(reason): _fail("deconnecte : " + reason))

	_connection.connect_to_server(
		"127.0.0.1", port, "godot-test-%d" % (Time.get_ticks_msec() % 1000000), "mot-de-passe-jetable"
	)


func _process(_delta: float) -> bool:
	if Time.get_ticks_msec() / 1000.0 - _started > TIMEOUT_SECONDS:
		_fail("delai depasse en phase « %s »" % _phase)
		return true

	if _connection.state == Connection.State.IN_WORLD:
		_advance()

	if _checks.values().all(func(done): return done):
		print("\nToutes les verifications passent :")
		for name in _checks:
			print("  ✓ %s" % name)
		quit(0)
		return true
	return false


## Avance le scenario un pas a la fois, a cadence reduite.
func _advance() -> void:
	_steps += 1
	# Un pas sur vingt : le serveur borne la distance par unite de temps, un
	# deplacement a chaque image serait refuse pour vitesse excessive.
	if _steps % 20 != 0:
		return

	if not _checks["deplacement legitime accepte"]:
		_phase = "deplacement"
		_entered_at += Vector2(120, 0)
		_connection.request_move(_entered_at)
		_legit_moves += 1
		# Cinq pas sans un seul refus : le serveur les a tous retenus.
		if _legit_moves >= 5:
			_pass("deplacement legitime accepte")
	elif not _checks["saut refuse et position retablie"]:
		_phase = "saut interdit"
		_connection.request_move(Vector2(1_000_000, 1_000_000))
	else:
		_phase = "exploration"
		# Les creatures sont a l'ecart du point d'apparition : il faut aller
		# les chercher, ce qui prouve au passage que le deplacement fonctionne
		# sur la duree.
		_entered_at += Vector2(140, 30)
		_connection.request_move(_entered_at)


func _on_world_entered(id: int, world_position: Vector2) -> void:
	_entered_at = world_position
	_pass("entre dans le monde")
	print("  entite %d en (%d, %d)" % [id, world_position.x, world_position.y])


func _on_move_rejected(world_position: Vector2) -> void:
	# Le serveur reaffirme la position : le client s'y realigne, faute de quoi
	# tous ses pas suivants partiraient d'un point imaginaire.
	_entered_at = world_position
	if _checks["deplacement legitime accepte"]:
		_pass("saut refuse et position retablie")
	else:
		_fail("un pas legitime a ete refuse — encodage ou cadence suspecte")


func _on_entity_appeared(id: int, kind: int, world_position: Vector2) -> void:
	if kind == Protocol.KIND_CREATURE:
		_pass("creature reperee")
		print("  creature %d en (%d, %d)" % [id, world_position.x, world_position.y])


func _pass(name: String) -> void:
	if not _checks[name]:
		_checks[name] = true
		print("  ✓ %s" % name)


func _fail(reason: String) -> void:
	printerr("ECHEC : %s" % reason)
	for name in _checks:
		if not _checks[name]:
			printerr("  ✗ %s" % name)
	quit(1)


func _argument(name: String, fallback: String) -> String:
	var arguments := OS.get_cmdline_user_args()
	var index := arguments.find(name)
	if index >= 0 and index + 1 < arguments.size():
		return arguments[index + 1]
	return fallback
