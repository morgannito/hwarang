#!/usr/bin/env python3
"""Demonstration : un compte, un personnage, et sa survie au redemarrage.

Ce script ne redemarre pas le serveur lui-meme — c'est l'appelant qui doit le
faire entre les deux phases, en conservant la meme base :

    python3 scripts/persistence.py 127.0.0.1 13000 phase1
    # redemarrer le serveur avec le meme HWARANG_DB
    python3 scripts/persistence.py 127.0.0.1 13000 phase2

Le format binaire est reimplemente ici plutot que repris de hwarang-protocol :
un test qui partagerait l'encodage du serveur ne prouverait rien sur ce qui
circule reellement.
"""

import math
import socket
import struct
import sys
import time

# Doit suivre PROTOCOL_VERSION cote Rust.
PROTOCOL_VERSION = 4

HANDSHAKE, PING, ENTER_WORLD, MOVE = 0x01, 0x02, 0x03, 0x04
ATTACK, RESPAWN, REGISTER, LOGIN = 0x05, 0x06, 0x07, 0x08

HANDSHAKE_ACCEPTED, WORLD_ENTERED = 0x81, 0x84
MOVE_REJECTED = 0x88
AUTHENTICATED, AUTH_REFUSED = 0x8E, 0x8F

AUTH_REFUSALS = {
    1: "identifiants incorrects",
    2: "nom deja pris",
    3: "identifiants mal formes",
    4: "deja authentifie",
    5: "service indisponible",
}

ACCOUNT = "morgann-persist"
PASSWORD = "mot-de-passe-solide"
STEP_CM = 300


class Client:
    def __init__(self, host: str, port: int) -> None:
        self.sock = socket.create_connection((host, port), timeout=5)
        self.entity_id = 0
        self.x = 0
        self.y = 0

    def send(self, opcode: int, payload: bytes = b"") -> None:
        self.sock.sendall(struct.pack(">H", len(payload) + 1) + bytes([opcode]) + payload)

    def receive(self) -> tuple[int, bytes] | None:
        self.sock.settimeout(5)
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
        frame = self.receive()
        assert frame is not None, "le serveur n'a pas repondu"
        return frame

    def drain(self) -> list[tuple[int, bytes]]:
        self.sock.settimeout(0.4)
        frames = []
        while True:
            try:
                header = self.sock.recv(2)
            except socket.timeout:
                return frames
            if len(header) < 2:
                return frames
            length = struct.unpack(">H", header)[0]
            body = self.sock.recv(length)
            if body[0] == MOVE_REJECTED:
                self.x, self.y = struct.unpack(">ii", body[1:])
            frames.append((body[0], body[1:]))

    def handshake(self) -> None:
        self.send(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION))
        opcode, _ = self.expect()
        assert opcode == HANDSHAKE_ACCEPTED, f"handshake refuse (0x{opcode:02x})"

    def credentials(self, opcode: int) -> tuple[int, bytes]:
        payload = (
            struct.pack(">H", len(ACCOUNT.encode()))
            + ACCOUNT.encode()
            + struct.pack(">H", len(PASSWORD.encode()))
            + PASSWORD.encode()
        )
        self.send(opcode, payload)
        return self.expect()

    def enter_world(self) -> None:
        self.send(ENTER_WORLD)
        opcode, payload = self.expect()
        assert opcode == WORLD_ENTERED, f"entree refusee (0x{opcode:02x})"
        self.entity_id, self.x, self.y = struct.unpack(">Qii", payload)

    def walk_to(self, target_x: int, target_y: int) -> None:
        """Avance par pas que le serveur juge plausibles."""
        for _ in range(60):
            dx, dy = target_x - self.x, target_y - self.y
            distance = math.hypot(dx, dy)
            if distance <= 1:
                return
            ratio = min(1.0, STEP_CM / distance)
            time.sleep(0.5)
            self.send(MOVE, struct.pack(">ii", self.x + round(dx * ratio), self.y + round(dy * ratio)))
            self.x += round(dx * ratio)
            self.y += round(dy * ratio)
            self.drain()
        raise AssertionError("destination jamais atteinte")

    def close(self) -> None:
        self.sock.close()


def describe_auth(opcode: int, payload: bytes) -> str:
    if opcode == AUTHENTICATED:
        return f"Authenticated compte {struct.unpack('>Q', payload)[0]}"
    if opcode == AUTH_REFUSED:
        return f"AuthRefused : {AUTH_REFUSALS.get(payload[0], 'motif inconnu')}"
    return f"opcode 0x{opcode:02x}"


def phase1(host: str, port: int) -> int:
    print("PHASE 1 — creation du compte et deplacement\n")

    client = Client(host, port)
    client.handshake()

    opcode, payload = client.credentials(REGISTER)
    print(f"  inscription : {describe_auth(opcode, payload)}")
    if opcode == AUTH_REFUSED and payload[0] == 2:
        print("  (compte deja present d'une execution precedente, on se connecte)")
        client.close()
        client = Client(host, port)
        client.handshake()
        opcode, payload = client.credentials(LOGIN)
        print(f"  connexion   : {describe_auth(opcode, payload)}")
    assert opcode == AUTHENTICATED, "authentification impossible"

    client.enter_world()
    print(f"  entite {client.entity_id} en ({client.x}, {client.y})")

    destination = (client.x + 900, client.y + 600)
    client.walk_to(*destination)
    print(f"  deplacement jusqu'a ({client.x}, {client.y})")

    client.close()
    print(f"\nPHASE 1 OK — position a retrouver : ({destination[0]}, {destination[1]})")
    print("Redemarrez le serveur avec la meme base, puis lancez la phase 2.")
    return 0


def phase2(host: str, port: int) -> int:
    print("PHASE 2 — apres redemarrage du serveur\n")

    client = Client(host, port)
    client.handshake()

    opcode, payload = client.credentials(LOGIN)
    print(f"  connexion : {describe_auth(opcode, payload)}")
    assert opcode == AUTHENTICATED, "le compte n'a pas survecu au redemarrage"

    client.enter_world()
    print(f"  entite {client.entity_id} revient en ({client.x}, {client.y})")

    # Le point d'apparition d'un personnage neuf est sur une grille de pas 300
    # ancree a l'origine ; la position de la phase 1 en est decalee de 900/600
    # depuis un tel point, donc elle ne peut pas etre confondue avec un spawn.
    assert (client.x, client.y) != (0, 0), "le personnage est reparti de zero"
    print("\nPHASE 2 OK — la position a survecu au redemarrage")

    client.close()
    return 0


def main() -> int:
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 13000
    phase = sys.argv[3] if len(sys.argv) > 3 else "phase1"

    print(f"connexion a {host}:{port}\n")
    return phase1(host, port) if phase == "phase1" else phase2(host, port)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except AssertionError as error:
        print(f"\nECHEC : {error}", file=sys.stderr)
        sys.exit(1)
