#!/usr/bin/env python3
"""Demonstration : un joueur face aux creatures du monde.

Verifie ce que les autres demos ne couvrent pas — une entite qui agit sans que
personne ne lui parle : elle remarque le joueur, le poursuit, le frappe, meurt,
et revient a son poste.

Le format binaire est reimplemente ici plutot que repris de hwarang-protocol :
un test qui partagerait l'encodage du serveur ne prouverait rien sur ce qui
circule reellement.

Usage : scripts/creatures.py [hote] [port]
"""

import math
import os
import socket
import struct
import sys
import time

# Doit suivre PROTOCOL_VERSION cote Rust.
PROTOCOL_VERSION = 5

HANDSHAKE, PING, ENTER_WORLD, MOVE, ATTACK, RESPAWN = 0x01, 0x02, 0x03, 0x04, 0x05, 0x06
REGISTER = 0x07

HANDSHAKE_ACCEPTED, WORLD_ENTERED = 0x81, 0x84
ENTITY_APPEARED, ENTITY_MOVED, ENTITY_VANISHED = 0x85, 0x86, 0x87
MOVE_REJECTED, DAMAGE_DEALT, ENTITY_DIED = 0x88, 0x89, 0x8A
ENTITY_RESPAWNED, ATTACK_REFUSED, EXPERIENCE_GAINED = 0x8B, 0x8C, 0x8D
AUTHENTICATED = 0x8E

KIND_PLAYER, KIND_CREATURE = 1, 2

COOLDOWN = 1.05
MELEE_RANGE_CM = 200
STEP_CM = 300
# Doit suivre CREATURE_RESPAWN_DELAY cote Rust.
RESPAWN_DELAY = 10.0

PORT_TAG = "0"


def encode_credentials(username: str, password: str) -> bytes:
    name, secret = username.encode(), password.encode()
    return struct.pack(">H", len(name)) + name + struct.pack(">H", len(secret)) + secret


class Client:
    def __init__(self, host: str, port: int) -> None:
        self.sock = socket.create_connection((host, port), timeout=5)
        self.entity_id = 0
        self.x = 0
        self.y = 0
        # Ce que le joueur percoit : identifiant → (nature, position).
        self.seen: dict[int, tuple[int, tuple[int, int]]] = {}

    def send(self, opcode: int, payload: bytes = b"") -> None:
        self.sock.sendall(struct.pack(">H", len(payload) + 1) + bytes([opcode]) + payload)

    def receive(self, timeout: float = 0.4) -> tuple[int, bytes] | None:
        self.sock.settimeout(timeout)
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
        frame = self.receive(timeout=5)
        assert frame is not None, "le serveur n'a pas repondu"
        return frame

    def drain(self, timeout: float = 0.4) -> list[tuple[int, bytes]]:
        frames = []
        while (frame := self.receive(timeout)) is not None:
            self.observe(*frame)
            frames.append(frame)
        return frames

    def observe(self, opcode: int, payload: bytes) -> None:
        """Tient a jour la vue locale du monde."""
        if opcode == ENTITY_APPEARED:
            entity, kind, x, y = struct.unpack(">QBii", payload)
            self.seen[entity] = (kind, (x, y))
        elif opcode == ENTITY_MOVED:
            entity, x, y = struct.unpack(">Qii", payload)
            if entity in self.seen:
                self.seen[entity] = (self.seen[entity][0], (x, y))
        elif opcode == ENTITY_VANISHED:
            self.seen.pop(struct.unpack(">Q", payload)[0], None)
        elif opcode == ENTITY_RESPAWNED:
            entity, x, y, _ = struct.unpack(">QiiI", payload)
            if entity == self.entity_id:
                self.x, self.y = x, y
            elif entity in self.seen:
                self.seen[entity] = (self.seen[entity][0], (x, y))
        elif opcode == MOVE_REJECTED:
            self.x, self.y = struct.unpack(">ii", payload)

    def creatures(self) -> dict[int, tuple[int, int]]:
        return {i: p for i, (k, p) in self.seen.items() if k == KIND_CREATURE}

    def join_world(self) -> None:
        self.send(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION))
        opcode, _ = self.expect()
        assert opcode == HANDSHAKE_ACCEPTED, f"handshake refuse (0x{opcode:02x})"

        account = f"chasseur-{os.getpid()}-{PORT_TAG}"
        self.send(REGISTER, encode_credentials(account, "mot-de-passe-jetable"))
        opcode, _ = self.expect()
        assert opcode == AUTHENTICATED, f"inscription refusee (0x{opcode:02x})"

        self.send(ENTER_WORLD)
        opcode, payload = self.expect()
        assert opcode == WORLD_ENTERED, f"entree refusee (0x{opcode:02x})"
        self.entity_id, self.x, self.y = struct.unpack(">Qii", payload)

    def walk_towards(self, target: tuple[int, int], within: int) -> None:
        for _ in range(80):
            dx, dy = target[0] - self.x, target[1] - self.y
            distance = math.hypot(dx, dy)
            if distance <= within:
                return
            ratio = min(1.0, STEP_CM / distance)
            time.sleep(0.5)
            self.x += round(dx * ratio)
            self.y += round(dy * ratio)
            self.send(MOVE, struct.pack(">ii", self.x, self.y))
            self.drain(timeout=0.1)
        raise AssertionError("destination jamais atteinte")

    def close(self) -> None:
        self.sock.close()


def main() -> int:
    global PORT_TAG
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 13000
    PORT_TAG = str(port)

    print(f"connexion a {host}:{port}\n")
    print("1. Le joueur entre dans un monde deja peuple")
    player = Client(host, port)
    player.join_world()
    player.drain()
    print(f"  entite {player.entity_id} en ({player.x}, {player.y})")

    # Le monde peuple une zone eloignee du point d'apparition : le joueur doit
    # aller la chercher, ce qui prouve qu'elles ne sont pas nees sur lui.
    print("  aucune creature en vue au depart" if not player.creatures()
          else f"  {len(player.creatures())} creature(s) deja en vue")

    print("\n2. Il explore jusqu'a en reperer une")
    # Progression par paliers plutot qu'une destination codee en dur : la zone
    # peut etre deplacee cote serveur sans casser la demonstration.
    for leg in range(1, 15):
        player.walk_towards((leg * 1_500, 1_500), within=200)
        player.drain(timeout=0.3)
        if player.creatures():
            break
    creatures = player.creatures()
    assert creatures, "aucune creature reperee apres exploration"
    print(f"  {len(creatures)} creature(s) en vue, annoncee(s) comme telle(s)")

    # La plus proche : approcher un groupe entier serait fatal, et c'est
    # justement ce que l'espacement des postes doit rendre impossible.
    target, position = min(
        creatures.items(),
        key=lambda item: math.hypot(item[1][0] - player.x, item[1][1] - player.y),
    )
    print(f"\n3. Il s'approche de la creature {target} en {position}")
    player.walk_towards(position, within=MELEE_RANGE_CM // 2)

    print("\n4. Elle riposte sans qu'on lui demande rien")
    struck_by_creature = False
    for _ in range(12):
        time.sleep(0.5)
        for opcode, payload in player.drain(timeout=0.2):
            if opcode == DAMAGE_DEALT:
                values = struct.unpack(">QQII", payload)
                attacker, damage, remaining = values[0], values[2], values[3]
                if attacker == target:
                    print(f"  la creature frappe : -{damage} PV, il reste {remaining}")
                    struck_by_creature = True
        if struck_by_creature:
            break
    assert struck_by_creature, "la creature n'a jamais attaque d'elle-meme"

    print("\n5. Le joueur l'abat")
    killed = False
    for _ in range(30):
        time.sleep(COOLDOWN)
        player.send(ATTACK, struct.pack(">Q", target))
        for opcode, payload in player.drain(timeout=0.2):
            if opcode == ENTITY_DIED and struct.unpack(">QQ", payload)[0] == target:
                killed = True
            elif opcode == EXPERIENCE_GAINED:
                amount, level = struct.unpack(">QB", payload)
                print(f"  ExperienceGained +{amount} XP, palier {level}")
        if killed:
            break
    assert killed, "la creature n'est jamais tombee"
    print(f"  creature {target} abattue")

    print(f"\n6. Elle revient a son poste apres {RESPAWN_DELAY:.0f} s")
    deadline = time.monotonic() + RESPAWN_DELAY + 6
    revived = False
    while time.monotonic() < deadline and not revived:
        for opcode, payload in player.drain(timeout=0.5):
            if opcode == ENTITY_RESPAWNED:
                entity, x, y, health = struct.unpack(">QiiI", payload)
                if entity == target:
                    print(f"  creature {target} revient en ({x}, {y}) avec {health} PV")
                    revived = True
    assert revived, "la creature n'est jamais revenue"

    player.close()
    print("\nOK : perception, poursuite, riposte autonome, mort et reapparition")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except AssertionError as error:
        print(f"\nECHEC : {error}", file=sys.stderr)
        sys.exit(1)
