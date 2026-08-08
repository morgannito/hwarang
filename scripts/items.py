#!/usr/bin/env python3
"""Demonstration : butin, sac et equipement.

Un joueur abat une creature, recupere ce qu'elle laisse, l'equipe, et constate
que ses coups portent plus fort. Puis il se deconnecte et retrouve son bagage.

Ce script ne redemarre pas le serveur : l'appelant le fait entre les deux phases,
en conservant la meme base.

    python3 scripts/items.py 127.0.0.1 13000 phase1
    # redemarrer le serveur avec le meme HWARANG_DB
    python3 scripts/items.py 127.0.0.1 13000 phase2

Le format binaire est reimplemente ici plutot que repris de hwarang-protocol.
"""

import math
import socket
import struct
import sys
import time

# Doit suivre PROTOCOL_VERSION cote Rust.
PROTOCOL_VERSION = 6

HANDSHAKE, PING, ENTER_WORLD, MOVE, ATTACK, RESPAWN = 0x01, 0x02, 0x03, 0x04, 0x05, 0x06
REGISTER, LOGIN, EQUIP_ITEM, UNEQUIP_ITEM = 0x07, 0x08, 0x09, 0x0A

HANDSHAKE_ACCEPTED, WORLD_ENTERED = 0x81, 0x84
ENTITY_APPEARED, ENTITY_MOVED, ENTITY_VANISHED = 0x85, 0x86, 0x87
MOVE_REJECTED, DAMAGE_DEALT, ENTITY_DIED = 0x88, 0x89, 0x8A
ENTITY_RESPAWNED, ATTACK_REFUSED, EXPERIENCE_GAINED = 0x8B, 0x8C, 0x8D
AUTHENTICATED, AUTH_REFUSED = 0x8E, 0x8F
ITEM_RECEIVED, INVENTORY_FULL, EQUIPMENT_CHANGED, EQUIP_REFUSED = 0x90, 0x91, 0x92, 0x93

KIND_CREATURE = 2
SLOT_WEAPON = 1

ACCOUNT = "morgann-butin"
PASSWORD = "mot-de-passe-solide"
COOLDOWN = 1.05
MELEE_RANGE_CM = 200
# Doit suivre CREATURE_RESPAWN_DELAY cote Rust.
RESPAWN_DELAY = 10.0
STEP_CM = 300


class Client:
    def __init__(self, host: str, port: int) -> None:
        self.sock = socket.create_connection((host, port), timeout=5)
        self.entity_id = 0
        self.x = 0
        self.y = 0
        self.seen: dict[int, tuple[int, tuple[int, int]]] = {}
        self.bag: dict[int, int] = {}
        self.worn: dict[int, int] = {}

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
        body = self.sock.recv(struct.unpack(">H", header)[0])
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
        if opcode == ENTITY_APPEARED:
            entity, kind, x, y = struct.unpack(">QBii", payload)
            self.seen[entity] = (kind, (x, y))
        elif opcode == ENTITY_MOVED:
            entity, x, y = struct.unpack(">Qii", payload)
            if entity in self.seen:
                self.seen[entity] = (self.seen[entity][0], (x, y))
        elif opcode == ENTITY_VANISHED:
            self.seen.pop(struct.unpack(">Q", payload)[0], None)
        elif opcode == MOVE_REJECTED:
            self.x, self.y = struct.unpack(">ii", payload)
        elif opcode == ITEM_RECEIVED:
            item, index = struct.unpack(">IH", payload)
            self.bag[index] = item
        elif opcode == EQUIPMENT_CHANGED:
            slot, item = struct.unpack(">BI", payload)
            if item == 0:
                self.worn.pop(slot, None)
            else:
                self.worn[slot] = item

    def creatures(self) -> dict[int, tuple[int, int]]:
        return {i: p for i, (k, p) in self.seen.items() if k == KIND_CREATURE}

    def connect_account(self, opcode: int) -> int:
        self.send(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION))
        assert self.expect()[0] == HANDSHAKE_ACCEPTED, "handshake refuse"
        name, secret = ACCOUNT.encode(), PASSWORD.encode()
        self.send(
            opcode,
            struct.pack(">H", len(name)) + name + struct.pack(">H", len(secret)) + secret,
        )
        return self.expect()[0]

    def enter_world(self) -> None:
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


def find_creature(player: Client) -> tuple[int, tuple[int, int]]:
    """Explore jusqu'a reperer une creature, puis la rejoint."""
    for leg in range(1, 15):
        player.walk_towards((leg * 1_500, 1_500), within=200)
        player.drain(timeout=0.3)
        if player.creatures():
            break
    creatures = player.creatures()
    assert creatures, "aucune creature reperee"

    target, position = min(
        creatures.items(),
        key=lambda item: math.hypot(item[1][0] - player.x, item[1][1] - player.y),
    )
    player.walk_towards(position, within=MELEE_RANGE_CM // 2)
    player.drain()
    return target, position


def kill(player: Client, target: int) -> int:
    """Poursuit une cible et l'abat. Retourne les degats par coup.

    La poursuite fait partie du combat : une creature engagee se deplace vers le
    joueur, qui doit rester a portee entre deux coups plutot que de frapper dans
    le vide depuis l'endroit ou il l'a rencontree.
    """
    damage_per_blow = 0
    for _ in range(30):
        time.sleep(COOLDOWN)
        player.send(ATTACK, struct.pack(">Q", target))
        killed = False
        out_of_range = False
        for opcode, payload in player.drain(timeout=0.2):
            if opcode == DAMAGE_DEALT:
                attacker, _, damage, _ = struct.unpack(">QQII", payload)
                if attacker == player.entity_id:
                    damage_per_blow = damage
            elif opcode == ENTITY_DIED and struct.unpack(">QQ", payload)[0] == target:
                killed = True
            elif opcode == ATTACK_REFUSED and payload[0] == 1:
                out_of_range = True
        if killed:
            return damage_per_blow
        if out_of_range and target in player.seen:
            player.walk_towards(player.seen[target][1], within=MELEE_RANGE_CM // 2)
    raise AssertionError("la creature n'est jamais tombee")


def await_respawn(player: Client, target: int, delay: float) -> tuple[int, int]:
    """Attend qu'une creature abattue revienne, et donne son poste.

    Elle reapparait a son **poste**, pas la ou elle est tombee : elle avait
    poursuivi le joueur, qui doit donc revenir la chercher.
    """
    deadline = time.monotonic() + delay + 6
    while time.monotonic() < deadline:
        for opcode, payload in player.drain(timeout=0.5):
            if opcode == ENTITY_RESPAWNED:
                entity, x, y, _ = struct.unpack(">QiiI", payload)
                if entity == target:
                    player.seen[entity] = (KIND_CREATURE, (x, y))
                    return (x, y)
    raise AssertionError("la creature n'est jamais revenue")


def phase1(host: str, port: int) -> int:
    print("PHASE 1 — butin et equipement\n")
    player = Client(host, port)

    opcode = player.connect_account(REGISTER)
    if opcode == AUTH_REFUSED:
        player.close()
        player = Client(host, port)
        opcode = player.connect_account(LOGIN)
    assert opcode == AUTHENTICATED, "authentification impossible"
    player.enter_world()
    print(f"  entite {player.entity_id} en ({player.x}, {player.y})")

    print("\n1. Il abat une creature et ramasse ce qu'elle laisse")
    target, _ = find_creature(player)
    before = kill(player, target)
    player.drain(timeout=0.5)
    assert player.bag, "aucun butin recu"
    index, item = next(iter(player.bag.items()))
    print(f"  degats a mains nues : {before}")
    print(f"  butin : objet {item} range a l'emplacement {index}")

    print("\n2. Il equipe l'objet")
    player.send(EQUIP_ITEM, struct.pack(">H", index))
    time.sleep(0.3)
    player.drain()
    assert player.worn, f"l'objet {item} n'a pas pu etre equipe"
    print(f"  equipement porte : {player.worn}")

    print("\n3. La creature revient ; ses coups portent plus fort")
    # La meme cible plutot qu'une nouvelle : elle revient a portee, et la
    # comparaison porte sur des adversaires identiques.
    post = await_respawn(player, target, RESPAWN_DELAY)
    player.walk_towards(post, within=MELEE_RANGE_CM // 2)
    player.drain()
    after = kill(player, target)
    print(f"  degats equipe : {after} (contre {before} a mains nues)")
    assert after > before, "l'equipement n'a rien change aux degats"

    player.close()
    print("\nPHASE 1 OK — redemarrez le serveur, puis lancez la phase 2.")
    return 0


def phase2(host: str, port: int) -> int:
    print("PHASE 2 — le bagage a-t-il survecu au redemarrage ?\n")
    player = Client(host, port)
    assert player.connect_account(LOGIN) == AUTHENTICATED, "compte introuvable"
    player.enter_world()
    player.drain(timeout=0.5)

    # L'equipement porte n'est pas retransmis a la connexion : on le constate
    # par ses effets, en frappant. Le sac, lui, se verifie en le vidant.
    print("  reconnexion reussie")
    player.send(UNEQUIP_ITEM, struct.pack(">B", SLOT_WEAPON))
    time.sleep(0.4)
    frames = player.drain()

    changed = [op for op, _ in frames if op == EQUIPMENT_CHANGED]
    refused = [op for op, _ in frames if op == EQUIP_REFUSED]
    assert changed, f"aucune arme portee apres redemarrage (refus : {len(refused)})"
    print("  l'arme etait toujours equipee : elle vient d'etre rangee au sac")

    player.close()
    print("\nPHASE 2 OK — le bagage a survecu au redemarrage")
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
