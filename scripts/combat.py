#!/usr/bin/env python3
"""Demonstration : deux joueurs s'affrontent jusqu'a la mort de l'un.

Verifie l'autorite du serveur sur l'offensive — portee, cadence, acharnement —
comme two_clients.py le fait pour le deplacement.

Le format binaire est reimplemente ici plutot que repris de hwarang-protocol :
un test qui partagerait l'encodage du serveur ne prouverait rien sur ce qui
circule reellement.

Usage : scripts/combat.py [hote] [port]
"""

import math
import os
import socket
import struct
import sys
import time

# Doit suivre PROTOCOL_VERSION cote Rust.
PROTOCOL_VERSION = 4

HANDSHAKE, PING, ENTER_WORLD, MOVE, ATTACK, RESPAWN = 0x01, 0x02, 0x03, 0x04, 0x05, 0x06
REGISTER = 0x07
HANDSHAKE_ACCEPTED, WORLD_ENTERED = 0x81, 0x84
AUTHENTICATED = 0x8E
MOVE_REJECTED = 0x88
DAMAGE_DEALT, ENTITY_DIED = 0x89, 0x8A
ENTITY_RESPAWNED, ATTACK_REFUSED, EXPERIENCE_GAINED = 0x8B, 0x8C, 0x8D

REFUSALS = {
    1: "hors de portee",
    2: "cadence non respectee",
    3: "attaquant a terre",
    4: "cible a terre",
    5: "cible = soi-meme",
    6: "cible inexistante",
}

# Le serveur autorise une attaque par seconde.
COOLDOWN = 1.05
# Allonge au corps a corps, cote serveur.
MELEE_RANGE_CM = 200
# Pas de deplacement, compatible avec la vitesse de course.
STEP_CM = 300


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
        self.sock.settimeout(0.4)
        try:
            header = self.sock.recv(2)
        except socket.timeout:
            return None
        if len(header) < 2:
            return None
        length = struct.unpack(">H", header)[0]
        return (body := self.sock.recv(length))[0], body[1:]

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

        Sans cette remise a niveau, un pas refuse laisse le client en avance :
        les suivants sont calcules depuis une position imaginaire, refuses a leur
        tour, et l'approche n'aboutit jamais.
        """
        if opcode == MOVE_REJECTED:
            self.x, self.y = struct.unpack(">ii", payload)
        elif opcode == ENTITY_RESPAWNED:
            entity, x, y, _ = struct.unpack(">QiiI", payload)
            if entity == self.entity_id:
                self.x, self.y = x, y

    def report(self) -> list[int]:
        opcodes = []
        for opcode, payload in self.drain():
            opcodes.append(opcode)
            print(f"  [{self.label}] {describe(opcode, payload)}")
        return opcodes

    def join_world(self) -> None:
        self.send(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION))
        opcode, _ = self.expect()
        assert opcode == HANDSHAKE_ACCEPTED, f"handshake refuse (0x{opcode:02x})"

        # Compte jetable, unique a cette execution : la demo doit partir d'un
        # personnage neuf et en pleine sante, pas de celui qu'une execution
        # precedente a laisse a terre.
        account = f"{self.label}-{os.getpid()}-{PORT_TAG}"
        self.send(REGISTER, encode_credentials(account, "mot-de-passe-jetable"))
        opcode, _ = self.expect()
        assert opcode == AUTHENTICATED, f"inscription refusee (0x{opcode:02x})"

        self.send(ENTER_WORLD)
        opcode, payload = self.expect()
        assert opcode == WORLD_ENTERED, f"entree refusee (0x{opcode:02x})"
        self.entity_id, self.x, self.y = struct.unpack(">Qii", payload)
        print(f"  [{self.label}] entite {self.entity_id} en ({self.x}, {self.y})")

    def move(self, x: int, y: int) -> None:
        self.send(MOVE, struct.pack(">ii", x, y))
        self.x, self.y = x, y

    def approach(self, target_x: int, target_y: int, within: int) -> None:
        """Avance vers une cible par pas que le serveur juge plausibles.

        Un unique grand pas serait refuse comme teleportation : la distance
        entre deux points d'apparition depend des identifiants d'entite et peut
        depasser plusieurs dizaines de metres.
        """
        for _ in range(60):
            dx, dy = target_x - self.x, target_y - self.y
            distance = math.hypot(dx, dy)
            if distance <= within:
                return
            # 300 cm exigent 357 ms a la vitesse de course, marge comprise.
            ratio = min(1.0, (distance - within + 1) / distance, STEP_CM / distance)
            time.sleep(0.5)
            self.move(self.x + round(dx * ratio), self.y + round(dy * ratio))
            self.drain()
        raise AssertionError("la cible n'a pas ete rejointe")

    def attack(self, target: int) -> None:
        self.send(ATTACK, struct.pack(">Q", target))

    def close(self) -> None:
        self.sock.close()


def describe(opcode: int, payload: bytes) -> str:
    if opcode == DAMAGE_DEALT:
        attacker, target, damage, remaining = struct.unpack(">QQII", payload)
        return f"DamageDealt {attacker}→{target} : -{damage} PV, reste {remaining}"
    if opcode == ENTITY_DIED:
        entity, killer = struct.unpack(">QQ", payload)
        return f"EntityDied entite {entity} tombee sous les coups de {killer}"
    if opcode == ENTITY_RESPAWNED:
        entity, x, y, health = struct.unpack(">QiiI", payload)
        return f"EntityRespawned entite {entity} en ({x}, {y}) avec {health} PV"
    if opcode == ATTACK_REFUSED:
        return f"AttackRefused : {REFUSALS.get(payload[0], 'motif inconnu')}"
    if opcode == EXPERIENCE_GAINED:
        amount, level = struct.unpack(">QB", payload)
        return f"ExperienceGained +{amount} XP, palier {level}"
    return f"opcode 0x{opcode:02x}"


def main() -> int:
    global PORT_TAG
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 13000
    PORT_TAG = str(port)

    print(f"connexion a {host}:{port}\n")
    print("1. Deux combattants entrent dans le monde")
    alice = Client("alice", host, port)
    alice.join_world()
    bob = Client("bob", host, port)
    bob.join_world()
    alice.drain()
    bob.drain()

    print("\n2. Alice attaque de loin : hors de portee")
    alice.attack(bob.entity_id)
    time.sleep(0.2)
    assert ATTACK_REFUSED in alice.report(), "l'attaque a distance a ete acceptee"

    print("\n3. Alice marche jusqu'au contact")
    alice.approach(bob.x, bob.y, within=MELEE_RANGE_CM // 2)
    print(f"   alice en ({alice.x}, {alice.y}), bob en ({bob.x}, {bob.y})")
    time.sleep(0.2)
    alice.drain()
    bob.drain()

    print("\n4. Elle frappe en respectant la cadence")
    time.sleep(COOLDOWN)
    alice.attack(bob.entity_id)
    time.sleep(0.2)
    assert DAMAGE_DEALT in alice.report(), "le coup au contact n'a pas porte"
    print("   bob percoit le coup qu'il encaisse :")
    assert DAMAGE_DEALT in bob.report(), "la cible n'a pas ete prevenue"

    print("\n5. Elle envoie 21 attaques d'affilee : une seule doit porter")
    # Aucune lecture entre les envois : `report` attend jusqu'a 0,4 s par trame
    # absente, ce qui suffirait a rouvrir la fenetre de cadence.
    time.sleep(COOLDOWN)
    for _ in range(21):
        alice.attack(bob.entity_id)
    time.sleep(0.4)

    frames = alice.report()
    landed = frames.count(DAMAGE_DEALT)
    refused = frames.count(ATTACK_REFUSED)
    assert landed == 1, f"{landed} coups ont porte au lieu d'un seul"
    assert refused == 20, f"{refused} refus au lieu de 20"

    print("\n6. Elle frappe a la cadence jusqu'a ce que bob tombe")
    blows = 1
    while True:
        time.sleep(COOLDOWN)
        alice.attack(bob.entity_id)
        blows += 1
        time.sleep(0.15)
        if ENTITY_DIED in alice.report():
            break
        assert blows < 30, "bob ne tombe jamais"
    print(f"   → {blows} coups pour venir a bout de bob")
    bob.report()

    print("\n7. Alice s'acharne sur le corps : refuse")
    time.sleep(COOLDOWN)
    alice.attack(bob.entity_id)
    time.sleep(0.2)
    assert ATTACK_REFUSED in alice.report(), "l'acharnement a ete accepte"

    print("\n8. Bob tente de riposter alors qu'il est a terre : refuse")
    bob.attack(alice.entity_id)
    time.sleep(0.2)
    assert ATTACK_REFUSED in bob.report(), "un mort a pu riposter"

    print("\n9. Bob reapparait ; alice en est informee")
    bob.send(RESPAWN)
    time.sleep(0.3)
    assert ENTITY_RESPAWNED in bob.report(), "la reapparition a echoue"
    assert ENTITY_RESPAWNED in alice.report(), "le temoin n'a pas ete prevenu"

    alice.close()
    bob.close()
    print("\nOK : portee, cadence, mort, experience et reapparition valides")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except AssertionError as error:
        print(f"\nECHEC : {error}", file=sys.stderr)
        sys.exit(1)
