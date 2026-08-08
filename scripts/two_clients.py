#!/usr/bin/env python3
"""Demonstration : deux clients dans le monde, chacun voyant l'autre bouger.

Verifie ce que le smoke test ne couvre pas — la diffusion entre connexions et
l'autorite du serveur sur les deplacements. Comme scripts/smoke.py, ce client
reimplemente le format binaire : partager l'encodage du serveur ne prouverait
rien sur ce qui circule reellement.

Usage : scripts/two_clients.py [hote] [port]
"""

import socket
import struct
import sys
import time

PROTOCOL_VERSION = 2

HANDSHAKE, PING, ENTER_WORLD, MOVE = 0x01, 0x02, 0x03, 0x04
HANDSHAKE_ACCEPTED = 0x81
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


class Client:
    def __init__(self, label: str, host: str, port: int) -> None:
        self.label = label
        self.sock = socket.create_connection((host, port), timeout=3)
        self.entity_id: int | None = None

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

    def drain(self) -> list[tuple[int, bytes]]:
        frames = []
        while (frame := self.receive()) is not None:
            frames.append(frame)
        return frames

    def report(self) -> list[int]:
        """Vide la boite de reception en journalisant, renvoie les opcodes vus."""
        opcodes = []
        for opcode, payload in self.drain():
            opcodes.append(opcode)
            print(f"  [{self.label}] {describe(opcode, payload)}")
        return opcodes

    def handshake_and_enter(self) -> None:
        self.send(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION))
        opcode, payload = self.receive()
        assert opcode == HANDSHAKE_ACCEPTED, f"handshake refuse (0x{opcode:02x})"

        self.send(ENTER_WORLD)
        opcode, payload = self.receive()
        assert opcode == WORLD_ENTERED, f"entree refusee (0x{opcode:02x})"
        self.entity_id, x, y = struct.unpack(">Qii", payload)
        print(f"  [{self.label}] entite {self.entity_id} apparait en ({x}, {y})")

    def move(self, x: int, y: int) -> None:
        self.send(MOVE, struct.pack(">ii", x, y))

    def close(self) -> None:
        self.sock.close()


def describe(opcode: int, payload: bytes) -> str:
    name = NAMES.get(opcode, f"opcode 0x{opcode:02x}")
    if opcode in (WORLD_ENTERED, ENTITY_APPEARED, ENTITY_MOVED):
        entity_id, x, y = struct.unpack(">Qii", payload)
        return f"{name} entite={entity_id} ({x}, {y})"
    if opcode == ENTITY_VANISHED:
        return f"{name} entite={struct.unpack('>Q', payload)[0]}"
    if opcode == MOVE_REJECTED:
        x, y = struct.unpack(">ii", payload)
        return f"{name} position retablie ({x}, {y})"
    return name


def main() -> int:
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 13000

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
    bob.move(700, 0)
    time.sleep(0.2)
    assert ENTITY_MOVED in alice.report(), "le deplacement de bob n'a pas ete diffuse"
    bob.drain()

    print("\n4. Bob tente un saut de 10 km : le serveur refuse et le replace")
    bob.move(1_000_000, 0)
    time.sleep(0.2)
    assert MOVE_REJECTED in bob.report(), "le saut n'a pas ete refuse"
    assert not alice.report(), "alice a percu un deplacement refuse"

    print("\n5. Bob s'eloigne par petits pas jusqu'a sortir du champ de vision")
    x = 700
    for _ in range(90):
        x += 300
        bob.move(x, 0)
        time.sleep(0.05)
        bob.drain()
    time.sleep(0.2)
    assert ENTITY_VANISHED in alice.report(), "bob n'a jamais disparu du champ"

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
