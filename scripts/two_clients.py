#!/usr/bin/env python3
"""Demonstration : deux clients dans le monde, chacun voyant l'autre bouger.

Verifie ce que le smoke test ne couvre pas — la diffusion entre connexions et
l'autorite du serveur sur les deplacements. Comme scripts/smoke.py, ce client
reimplemente le format binaire : partager l'encodage du serveur ne prouverait
rien sur ce qui circule reellement.

Usage : scripts/two_clients.py [hote] [port]
"""

import os
import socket
import struct
import sys
import time

# Doit suivre PROTOCOL_VERSION cote Rust.
PROTOCOL_VERSION = 6

HANDSHAKE, PING, ENTER_WORLD, MOVE = 0x01, 0x02, 0x03, 0x04
REGISTER = 0x07
HANDSHAKE_ACCEPTED = 0x81
AUTHENTICATED = 0x8E
WORLD_ENTERED, ENTITY_APPEARED = 0x84, 0x85
ENTITY_MOVED, ENTITY_VANISHED, MOVE_REJECTED = 0x86, 0x87, 0x88

NAMES = {
    HANDSHAKE_ACCEPTED: "HandshakeAccepted",
    WORLD_ENTERED: "WorldEntered",
    ENTITY_APPEARED: "EntityAppeared",
    ENTITY_MOVED: "EntityMoved",
    ENTITY_VANISHED: "EntityVanished",
    MOVE_REJECTED: "MoveRejected",
}


# Distingue deux executions simultanees sur des serveurs differents.
PORT_TAG = "0"


def encode_credentials(username: str, password: str) -> bytes:
    """Deux chaines precedees chacune de leur longueur en octets."""
    name, secret = username.encode(), password.encode()
    return struct.pack(">H", len(name)) + name + struct.pack(">H", len(secret)) + secret


class Client:
    def __init__(self, label: str, host: str, port: int) -> None:
        self.label = label
        self.sock = socket.create_connection((host, port), timeout=3)
        self.entity_id = 0
        # Position reelle, lue du serveur. Le point d'apparition depend de
        # l'identifiant d'entite : coder une position en dur rend la demo
        # dependante du nombre de connexions deja servies.
        self.x = 0
        self.y = 0

    def send(self, opcode: int, payload: bytes = b"") -> None:
        self.sock.sendall(struct.pack(">H", len(payload) + 1) + bytes([opcode]) + payload)

    def receive(self) -> tuple[int, bytes] | None:
        """Lit une trame, ou None si rien n'arrive avant l'expiration."""
        self.sock.settimeout(0.4)
        try:
            header = self.sock.recv(2)
        except socket.timeout:
            return None
        if len(header) < 2:
            return None
        length = struct.unpack(">H", header)[0]
        body = self.sock.recv(length)
        return body[0], body[1:]

    def expect(self) -> tuple[int, bytes]:
        """Lit une trame la ou une reponse est certaine, ou echoue."""
        frame = self.receive()
        assert frame is not None, "le serveur n'a pas repondu"
        return frame

    def drain(self) -> list[tuple[int, bytes]]:
        frames = []
        while (frame := self.receive()) is not None:
            self.reconcile(*frame)
            frames.append(frame)
        return frames

    def reconcile(self, opcode: int, payload: bytes) -> None:
        """Aligne l'etat local sur ce que le serveur affirme.

        Un deplacement refuse laisse le client en avance sur le serveur. Sans
        cette remise a niveau, l'ecart ne se resorbe jamais : chaque pas suivant
        est calcule depuis une position imaginaire, donc refuse a son tour, et la
        derive s'aggrave. C'est exactement ce que `MoveRejected` sert a eviter,
        et pourquoi il transporte la position plutot qu'un simple refus.
        """
        if opcode == MOVE_REJECTED:
            self.x, self.y = struct.unpack(">ii", payload)

    def report(self) -> list[int]:
        """Vide la boite de reception en journalisant, renvoie les opcodes vus."""
        opcodes = []
        for opcode, payload in self.drain():
            opcodes.append(opcode)
            print(f"  [{self.label}] {describe(opcode, payload)}")
        return opcodes

    def handshake_and_enter(self) -> None:
        self.send(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION))
        opcode, _ = self.expect()
        assert opcode == HANDSHAKE_ACCEPTED, f"handshake refuse (0x{opcode:02x})"

        # Compte jetable, unique a cette execution : la demo doit partir d'un
        # personnage neuf, pas de celui qu'une execution precedente a deplace.
        account = f"{self.label}-{os.getpid()}-{PORT_TAG}"
        self.send(REGISTER, encode_credentials(account, "mot-de-passe-jetable"))
        opcode, _ = self.expect()
        assert opcode == AUTHENTICATED, f"inscription refusee (0x{opcode:02x})"

        self.send(ENTER_WORLD)
        opcode, payload = self.expect()
        assert opcode == WORLD_ENTERED, f"entree refusee (0x{opcode:02x})"
        self.entity_id, self.x, self.y = struct.unpack(">Qii", payload)
        print(f"  [{self.label}] entite {self.entity_id} apparait en ({self.x}, {self.y})")

    def move(self, x: int, y: int) -> None:
        self.send(MOVE, struct.pack(">ii", x, y))
        self.x, self.y = x, y

    def close(self) -> None:
        self.sock.close()


KINDS = {1: "joueur", 2: "creature"}


def describe(opcode: int, payload: bytes) -> str:
    name = NAMES.get(opcode, f"opcode 0x{opcode:02x}")
    if opcode == ENTITY_APPEARED:
        # Depuis la v5, l'apparition porte la nature de l'entite.
        entity_id, kind, x, y = struct.unpack(">QBii", payload)
        return f"{name} entite={entity_id} ({KINDS.get(kind, '?')}) ({x}, {y})"
    if opcode in (WORLD_ENTERED, ENTITY_MOVED):
        entity_id, x, y = struct.unpack(">Qii", payload)
        return f"{name} entite={entity_id} ({x}, {y})"
    if opcode == ENTITY_VANISHED:
        return f"{name} entite={struct.unpack('>Q', payload)[0]}"
    if opcode == MOVE_REJECTED:
        x, y = struct.unpack(">ii", payload)
        return f"{name} position retablie ({x}, {y})"
    return name


def main() -> int:
    global PORT_TAG
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 13000
    PORT_TAG = str(port)

    print(f"connexion a {host}:{port}\n")
    print("1. Deux joueurs entrent dans le monde")
    alice = Client("alice", host, port)
    alice.handshake_and_enter()
    bob = Client("bob", host, port)
    bob.handshake_and_enter()

    print("\n2. Ils se decouvrent mutuellement")
    seen_by_alice = alice.report()
    seen_by_bob = bob.report()
    assert ENTITY_APPEARED in seen_by_alice, "alice n'a pas vu bob apparaitre"
    assert ENTITY_APPEARED in seen_by_bob, "bob n'a pas vu alice apparaitre"

    print("\n3. Bob avance de 2 m ; alice doit le voir bouger")
    bob.move(bob.x + 200, bob.y)
    time.sleep(0.2)
    assert ENTITY_MOVED in alice.report(), "le deplacement de bob n'a pas ete diffuse"
    bob.drain()

    print("\n4. Bob tente un saut de 10 km : le serveur refuse et le replace")
    before = (bob.x, bob.y)
    bob.move(1_000_000, 0)
    time.sleep(0.2)
    # `reconcile` remet bob a la position que le serveur vient de reaffirmer.
    assert MOVE_REJECTED in bob.report(), "le saut n'a pas ete refuse"
    assert (bob.x, bob.y) == before, "le client ne s'est pas resynchronise"

    # L'assertion porte sur les deplacements de bob, pas sur le silence total :
    # d'autres entites peuvent entrer ou sortir du champ d'alice au meme moment,
    # et exiger une boite vide rendrait la demo dependante du voisinage.
    moves_of_bob = [
        opcode
        for opcode, payload in alice.drain()
        if opcode == ENTITY_MOVED and struct.unpack(">Qii", payload)[0] == bob.entity_id
    ]
    assert not moves_of_bob, "alice a percu un deplacement pourtant refuse"

    print("\n5. Bob s'eloigne par petits pas jusqu'a sortir du champ de vision")
    # La pause est explicite et non deleguee au `drain` : celui-ci rend la main
    # sans attendre des qu'une trame patiente, l'intervalle se resserre et le
    # serveur refuse les pas — bob n'avancerait jamais assez loin.
    # 300 cm exigent 357 ms a la vitesse de course, marge comprise.
    for step in range(1, 31):
        time.sleep(0.5)
        bob.move(bob.x + 300, bob.y)
        bob.drain()
        if ENTITY_VANISHED in [opcode for opcode, _ in alice.drain()]:
            print(f"  [alice] bob sort du champ apres {step} pas")
            break
    else:
        raise AssertionError("bob n'a jamais disparu du champ")

    print("\n6. Bob se deconnecte")
    bob.close()
    time.sleep(0.3)
    alice.report()
    alice.close()

    print("\nOK : diffusion, autorite serveur et champ de vision valides")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except AssertionError as error:
        print(f"\nECHEC : {error}", file=sys.stderr)
        sys.exit(1)
