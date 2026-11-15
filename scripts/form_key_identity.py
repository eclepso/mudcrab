"""Portable owner/local identities and authenticated current-world SQL lookup.

The digest encoding matches shared::pack_identity v1. This module never treats an
external integer as a stored key or invents a runtime allocation.
"""

import hashlib
import json
import re
import struct
import sqlite3


TAMRIEL = "00003C:Skyrim.esm"


def named(value, nullable=False):
    if value is None and nullable:
        return None
    if not isinstance(value, str):
        raise ValueError("identity must be an owner/local name, not a numeric ID")
    match = re.fullmatch(r"([0-9A-Fa-f]{1,6}):(.+)", value.strip())
    if not match:
        raise ValueError("identity must have the form 00003C:Skyrim.esm")
    plugin = match[2]
    if (plugin.strip() != plugin or any(ord(c) < 32 or ord(c) == 127 or c in '\\/:*?\"<>|' for c in plugin)
            or not re.fullmatch(r".+\.(esm|esp|esl)", plugin, re.IGNORECASE)):
        raise ValueError("invalid identity owner filename")
    return f"{int(match[1], 16):06X}:{plugin.lower()}"


def require_schema(document, field="schema_version", version=2):
    if type(document.get(field)) is not int or document[field] != version:
        raise ValueError(f"{field} must be {version}; numeric identity documents must be regenerated")


class PackIdentity:
    """Read-only SQL scope; validates allocation, proofs, priorities and digest."""

    def __init__(self, connection):
        try:
            version, expected = connection.execute("SELECT version,digest FROM pack_identity WHERE singleton=1").fetchone()
            rows = connection.execute("SELECT id,normalized_name,display_name,loaded,flags,header_version_bits,masters_json,content_hash,supplier_priority FROM pack_plugins ORDER BY id").fetchall()
        except (TypeError, ValueError, sqlite3.Error) as error:
            # SQLite versions report different missing-table exception subclasses.
            raise ValueError("world has no current plugin identity; convert again") from error
        if version != 1 or not isinstance(expected, bytes) or len(expected) != 32:
            raise ValueError("unsupported world identity; convert again")
        digest = hashlib.sha256(b"MudCrab pack identity v1\0")
        u32 = lambda value: struct.pack("<I", value)
        u64 = lambda value: struct.pack("<Q", value)
        def text(value):
            encoded = value.encode("utf-8")
            digest.update(u64(len(encoded)))
            digest.update(encoded)
        digest.update(u64(len(rows)))
        names, priorities = {}, {}
        for index, (owner, normalized, display, loaded, flags, bits, masters_json, proof, priority) in enumerate(rows):
            if owner != index + 1 or owner > 0xFFFFFFFF or named("0:" + display).split(":", 1)[1] != normalized or normalized in names:
                raise ValueError("invalid contiguous plugin allocation")
            if loaded not in (0, 1) or not isinstance(flags, int) or not 0 <= flags <= 7:
                raise ValueError("invalid stored plugin flags")
            masters = json.loads(masters_json)
            if not isinstance(masters, list) or any(not isinstance(item, str) for item in masters):
                raise ValueError("invalid source master list")
            if bits is not None and (type(bits) is not int or not 0 <= bits <= 0xFFFFFFFF):
                raise ValueError("invalid source header version")
            if proof is not None and (not isinstance(proof, bytes) or len(proof) != 32):
                raise ValueError("invalid source content proof")
            if bool(loaded) != (priority is not None) or loaded and proof is None:
                raise ValueError("incomplete source priority or proof")
            if priority is not None:
                if type(priority) is not int or priority < 0 or priority in priorities:
                    raise ValueError("invalid source priority")
                priorities[priority] = owner
            names[normalized] = owner
            digest.update(u32(owner))
            text(normalized)
            text(display)
            digest.update(bytes((loaded, bool(flags & 1), bool(flags & 2), bool(flags & 4), bits is not None)))
            digest.update(u32(bits or 0))
            digest.update(u64(len(masters)))
            for master in masters:
                text(master)
            digest.update(bytes((proof is not None,)))
            if proof is not None:
                digest.update(proof)
        if sorted(priorities) != list(range(len(priorities))):
            raise ValueError("noncontiguous source priorities")
        digest.update(u64(len(priorities)))
        for priority in range(len(priorities)):
            digest.update(u32(priorities[priority]))
        if digest.digest() != expected:
            raise ValueError("world identity proof does not match its table; convert again")
        self.names = names
        self.digest = expected.hex()

    def stored_key(self, value):
        local, plugin = named(value).split(":", 1)
        if plugin not in self.names:
            raise ValueError(f"identity owner is absent from this world: {plugin}")
        return self.names[plugin] << 24 | int(local, 16)
