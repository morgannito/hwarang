## Connexion au serveur Hwarang.
##
## Tient la socket, la machine a etats et le tampon de reception ; traduit les
## trames en signaux. Ne connait rien de l'affichage.
class_name Connection
extends Node

signal authenticated(account_id: int)
signal auth_refused(reason: int)
signal world_entered(entity_id: int, position: Vector2)
signal entity_appeared(entity_id: int, kind: int, position: Vector2)
signal entity_moved(entity_id: int, position: Vector2)
signal entity_vanished(entity_id: int)
## Le serveur refuse un deplacement et réaffirme la position qui fait foi.
signal move_rejected(position: Vector2)
signal damage_dealt(attacker: int, target: int, damage: int, remaining: int)
signal entity_died(entity_id: int, killer: int)
signal entity_respawned(entity_id: int, position: Vector2, health: int)
signal experience_gained(amount: int, level: int)
signal item_received(item: int, slot_index: int)
signal equipment_changed(slot: int, item: int)
signal notice(text: String)
signal disconnected(reason: String)

enum State { OFFLINE, HANDSHAKING, AUTHENTICATING, READY, IN_WORLD }

var state: State = State.OFFLINE
var entity_id: int = 0

var _socket := StreamPeerTCP.new()
var _buffer := PackedByteArray()
var _credentials := {}


func connect_to_server(host: String, port: int, username: String, password: String) -> void:
	_credentials = {"username": username, "password": password}
	var error := _socket.connect_to_host(host, port)
	if error != OK:
		disconnected.emit("connexion impossible : %s" % error_string(error))
		return
	state = State.HANDSHAKING


func _process(_delta: float) -> void:
	if state == State.OFFLINE:
		return

	_socket.poll()
	match _socket.get_status():
		StreamPeerTCP.STATUS_CONNECTED:
			if state == State.HANDSHAKING and _buffer.is_empty():
				_send(Protocol.HANDSHAKE, _version_payload())
				state = State.AUTHENTICATING
			_receive()
		StreamPeerTCP.STATUS_ERROR:
			_fail("connexion perdue")
		StreamPeerTCP.STATUS_NONE:
			if state != State.OFFLINE:
				_fail("connexion fermee par le serveur")


func _version_payload() -> PackedByteArray:
	var out := StreamPeerBuffer.new()
	out.big_endian = true
	out.put_u16(Protocol.VERSION)
	return out.data_array


func _receive() -> void:
	var available := _socket.get_available_bytes()
	if available > 0:
		var result: Array = _socket.get_data(available)
		if result[0] == OK:
			_buffer.append_array(result[1])

	# Un client qui laisserait son tampon croitre sans jamais completer de trame
	# reproduirait cote client le defaut qu'on a corrige cote serveur.
	if _buffer.size() > Protocol.MAX_FRAME_LEN:
		_fail("flux incoherent : tampon sature")
		return

	while true:
		var frame := Protocol.decode(_buffer)
		if frame.is_empty():
			return
		if frame.has("error"):
			_fail("trame invalide")
			return
		_buffer = _buffer.slice(frame["consumed"])
		_handle(frame["opcode"], frame["payload"])


func _handle(opcode: int, payload: PackedByteArray) -> void:
	var reader := Protocol.reader(payload)

	match opcode:
		Protocol.HANDSHAKE_ACCEPTED:
			# Le compte est cree s'il n'existe pas ; l'echec bascule sur une
			# connexion, ce qui rend le client utilisable sans inscription
			# prealable.
			_send(
				Protocol.REGISTER,
				Protocol.credentials(_credentials["username"], _credentials["password"])
			)
		Protocol.HANDSHAKE_REJECTED:
			_fail("version de protocole incompatible (serveur : %d)" % reader.get_u16())
		Protocol.AUTHENTICATED:
			state = State.READY
			authenticated.emit(reader.get_u64())
			_send(Protocol.ENTER_WORLD, Protocol.empty())
		Protocol.AUTH_REFUSED:
			var reason := reader.get_u8()
			# 2 = nom deja pris : le compte existe, on se connecte dessus.
			if reason == 2 and state == State.AUTHENTICATING:
				_send(
					Protocol.LOGIN,
					Protocol.credentials(_credentials["username"], _credentials["password"])
				)
			else:
				auth_refused.emit(reason)
		Protocol.WORLD_ENTERED:
			entity_id = reader.get_u64()
			state = State.IN_WORLD
			world_entered.emit(entity_id, Vector2(reader.get_32(), reader.get_32()))
		Protocol.ENTITY_APPEARED:
			var id := reader.get_u64()
			var kind := reader.get_u8()
			entity_appeared.emit(id, kind, Vector2(reader.get_32(), reader.get_32()))
		Protocol.ENTITY_MOVED:
			var id := reader.get_u64()
			entity_moved.emit(id, Vector2(reader.get_32(), reader.get_32()))
		Protocol.ENTITY_VANISHED:
			entity_vanished.emit(reader.get_u64())
		Protocol.MOVE_REJECTED:
			move_rejected.emit(Vector2(reader.get_32(), reader.get_32()))
		Protocol.DAMAGE_DEALT:
			damage_dealt.emit(
				reader.get_u64(), reader.get_u64(), reader.get_u32(), reader.get_u32()
			)
		Protocol.ENTITY_DIED:
			entity_died.emit(reader.get_u64(), reader.get_u64())
		Protocol.ENTITY_RESPAWNED:
			var id := reader.get_u64()
			entity_respawned.emit(
				id, Vector2(reader.get_32(), reader.get_32()), reader.get_u32()
			)
		Protocol.EXPERIENCE_GAINED:
			experience_gained.emit(reader.get_u64(), reader.get_u8())
		Protocol.ITEM_RECEIVED:
			item_received.emit(reader.get_u32(), reader.get_u16())
		Protocol.EQUIPMENT_CHANGED:
			equipment_changed.emit(reader.get_u8(), reader.get_u32())
		Protocol.INVENTORY_FULL:
			notice.emit("sac plein")
		Protocol.EQUIP_REFUSED:
			notice.emit("objet non equipable")
		Protocol.ATTACK_REFUSED:
			notice.emit(_refusal_text(reader.get_u8()))
		Protocol.PONG:
			pass
		_:
			notice.emit("trame inconnue 0x%02X" % opcode)


func _refusal_text(reason: int) -> String:
	match reason:
		1: return "hors de portee"
		2: return "trop tot"
		3: return "vous etes a terre"
		4: return "la cible est a terre"
		5: return "cible invalide"
		6: return "cible inexistante"
		_: return "attaque refusee"


func request_move(position: Vector2) -> void:
	if state == State.IN_WORLD:
		_send(Protocol.MOVE, Protocol.point(int(position.x), int(position.y)))


func request_attack(target: int) -> void:
	if state == State.IN_WORLD:
		_send(Protocol.ATTACK, Protocol.entity(target))


func request_respawn() -> void:
	if state == State.IN_WORLD:
		_send(Protocol.RESPAWN, Protocol.empty())


func request_equip(slot_index: int) -> void:
	if state == State.IN_WORLD:
		var out := StreamPeerBuffer.new()
		out.big_endian = true
		out.put_u16(slot_index)
		_send(Protocol.EQUIP_ITEM, out.data_array)


func _send(opcode: int, payload: PackedByteArray) -> void:
	_socket.put_data(Protocol.frame(opcode, payload))


func _fail(reason: String) -> void:
	state = State.OFFLINE
	_socket.disconnect_from_host()
	disconnected.emit(reason)
