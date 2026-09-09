#!/usr/bin/env python3
"""Walk the AT-SPI accessibility tree of a running slack-light window.

The predecessor's pty harness reconstructed the screen from the escape
stream and asserted on it. This is the GUI equivalent: the same tree a
screen reader reads, walked over D-Bus, printed as roles and names — and
therefore assertable. A control that is not in here is a defect twice over.

    tree.py [--app NAME] [--json] [--states]

Talks to the a11y bus directly (its address comes from org.a11y.Bus on the
session bus), with PyGObject's Gio and nothing else: no pyatspi, no atspi
crate, so a CI image needs only what GTK already needs.
"""
import json
import sys

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

ACC = "org.a11y.atspi.Accessible"

# The states worth printing. AT-SPI sends a 64-bit bitfield in two words;
# these are the ones a test asks about — "is the message cursor on this row"
# has no other observable answer.
STATES = {12: "focused", 23: "selected", 8: "enabled", 25: "showing"}
ASKED = (12, 23)


def a11y_bus():
    session = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    reply = session.call_sync(
        "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress",
        None, GLib.VariantType("(s)"), Gio.DBusCallFlags.NONE, 2000, None,
    )
    address = reply.unpack()[0]
    return Gio.DBusConnection.new_for_address_sync(
        address,
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
        | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
        None, None,
    )


def call(bus, name, path, iface, method, args=None, reply=None):
    return bus.call_sync(
        name, path, iface, method, args, reply, Gio.DBusCallFlags.NONE, 2000, None,
    ).unpack()


def prop(bus, name, path, key):
    try:
        v = call(
            bus, name, path, "org.freedesktop.DBus.Properties", "Get",
            GLib.Variant("(ss)", (ACC, key)), GLib.VariantType("(v)"),
        )
        return v[0]
    except GLib.Error:
        return None


def children(bus, name, path):
    try:
        return call(bus, name, path, ACC, "GetChildren", None, GLib.VariantType("(a(so))"))[0]
    except GLib.Error:
        return []


def states(bus, name, path):
    try:
        words = call(bus, name, path, ACC, "GetState", None, GLib.VariantType("(au)"))[0]
    except GLib.Error:
        return []
    bits = 0
    for i, w in enumerate(words):
        bits |= w << (32 * i)
    return [STATES[b] for b in ASKED if bits >> b & 1]


def role(bus, name, path):
    try:
        return call(bus, name, path, ACC, "GetRoleName", None, GLib.VariantType("(s)"))[0]
    except GLib.Error:
        return "?"


def walk(bus, name, path, depth=0, out=None, limit=1500, with_states=False):
    """Depth-first, bounded: a list of five thousand rows is virtualised,
    so the tree only ever holds the realised ones, but a runaway is still
    worth a ceiling."""
    if out is None:
        out = []
    if len(out) >= limit:
        return out
    node = {
        "depth": depth,
        "role": role(bus, name, path),
        "name": prop(bus, name, path, "Name") or "",
    }
    if with_states:
        node["states"] = states(bus, name, path)
    out.append(node)
    for cname, cpath in children(bus, name, path):
        walk(bus, cname, cpath, depth + 1, out, limit, with_states)
    return out


def main():
    want = "slack-light"
    as_json = "--json" in sys.argv
    if "--app" in sys.argv:
        want = sys.argv[sys.argv.index("--app") + 1]
    bus = a11y_bus()
    root = "/org/a11y/atspi/accessible/root"
    apps = children(bus, "org.a11y.atspi.Registry", root)
    found = []
    for name, path in apps:
        app_name = prop(bus, name, path, "Name") or ""
        if want.lower() in app_name.lower():
            found.append((name, path, app_name))
    if not found:
        names = [prop(bus, n, p, "Name") for n, p in apps]
        print(f"no application matching {want!r} on the a11y bus; present: {names}", file=sys.stderr)
        sys.exit(2)
    name, path, app_name = found[0]
    nodes = walk(bus, name, path, with_states="--states" in sys.argv)
    if as_json:
        print(json.dumps({"app": app_name, "nodes": nodes}, ensure_ascii=False))
    else:
        for n in nodes:
            label = f" {n['name']!r}" if n["name"] else ""
            st = f" [{','.join(n['states'])}]" if n.get("states") else ""
            print(f"{'  ' * n['depth']}{n['role']}{label}{st}")


if __name__ == "__main__":
    main()
