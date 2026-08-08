## Encodage et decodage des trames Hwarang.
##
## Reimplemente le format cote client. Format identique au serveur :
## `[longueur: u16 BE][opcode: u8][charge utile]`, la longueur couvrant l'opcode
## et la charge utile.
##
## Tout est en gros-boutiste, ce qui n'est pas le defaut de `PackedByteArray` :
## `StreamPeerBuffer.big_endian` doit etre arme explicitement, sinon chaque
## entier est lu a l'envers et le flux parait corrompu des la premiere trame.
class_name Protocol
extends RefCounted

const VERSION := 6

# Client vers serveur.
const HANDSHAKE := 0x01
const PING := 0x02
const ENTER_WORLD := 0x03
const MOVE := 0x04
const ATTACK := 0x05
const RESPAWN := 0x06
const REGISTER := 0x07
const LOGIN := 0x08
const EQUIP_ITEM := 0x09
const UNEQUIP_ITEM := 0x0A

# Serveur vers client.
const HANDSHAKE_ACCEPTED := 0x81
const HANDSHAKE_REJECTED := 0x82
const PONG := 0x83
const WORLD_ENTERED := 0x84
const ENTITY_APPEARED := 0x85
const ENTITY_MOVED := 0x86
const ENTITY_VANISHED := 0x87
const MOVE_REJECTED := 0x88
const DAMAGE_DEALT := 0x89
const ENTITY_DIED := 0x8A
const ENTITY_RESPAWNED := 0x8B
const ATTACK_REFUSED := 0x8C
const EXPERIENCE_GAINED := 0x8D
const AUTHENTICATED := 0x8E
const AUTH_REFUSED := 0x8F
const ITEM_RECEIVED := 0x90
const INVENTORY_FULL := 0x91
const EQUIPMENT_CHANGED := 0x92
const EQUIP_REFUSED := 0x93

const KIND_PLAYER := 1
const KIND_CREATURE := 2

## Plafond d'une trame, identique au serveur.
const MAX_FRAME_LEN := 8 * 1024


## Emballe une charge utile dans une trame complete.
static func frame(opcode: int, payload: PackedByteArray) -> PackedByteArray:
	var out := StreamPeerBuffer.new()
	out.big_endian = true
	out.put_u16(payload.size() + 1)
	out.put_u8(opcode)
	out.put_data(payload)
	return out.data_array


## Charge utile vide, pour les trames qui n'en portent pas.
static func empty() -> PackedByteArray:
	return PackedByteArray()


## Deux entiers signes, pour les positions.
static func point(x: int, y: int) -> PackedByteArray:
	var out := StreamPeerBuffer.new()
	out.big_endian = true
	out.put_32(x)
	out.put_32(y)
	return out.data_array


## Un entier non signe sur 64 bits, pour les identifiants d'entite.
static func entity(id: int) -> PackedByteArray:
	var out := StreamPeerBuffer.new()
	out.big_endian = true
	out.put_u64(id)
	return out.data_array


## Deux chaines precedees chacune de leur longueur en octets.
static func credentials(username: String, password: String) -> PackedByteArray:
	var out := StreamPeerBuffer.new()
	out.big_endian = true
	# Tableau type : sans annotation, GDScript n'infere pas le type des elements
	# et refuse de compiler l'appel a `to_utf8_buffer`.
	var texts: Array[String] = [username, password]
	for text in texts:
		var bytes := text.to_utf8_buffer()
		out.put_u16(bytes.size())
		out.put_data(bytes)
	return out.data_array


## Extrait la premiere trame complete d'un tampon.
##
## Retourne `{}` s'il manque des octets — un tampon incomplet est le cas normal
## avec TCP, pas une erreur. Sinon `{opcode, payload, consumed}`.
static func decode(buffer: PackedByteArray) -> Dictionary:
	if buffer.size() < 2:
		return {}

	var header := StreamPeerBuffer.new()
	header.big_endian = true
	header.data_array = buffer
	var announced := header.get_u16()

	if announced == 0 or announced > MAX_FRAME_LEN:
		# Trame impossible : le flux est corrompu, l'appelant doit couper plutot
		# que de tenter de se resynchroniser.
		return {"error": true}
	if buffer.size() < 2 + announced:
		return {}

	return {
		"opcode": buffer[2],
		"payload": buffer.slice(3, 2 + announced),
		"consumed": 2 + announced,
	}


## Lecteur sequentiel sur une charge utile.
##
## Les entiers sont lus en gros-boutiste, comme ils sont ecrits.
static func reader(payload: PackedByteArray) -> StreamPeerBuffer:
	var stream := StreamPeerBuffer.new()
	stream.big_endian = true
	stream.data_array = payload
	return stream
