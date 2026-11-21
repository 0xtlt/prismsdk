#!/usr/bin/env python3
"""A stand-in for a panel's StatusNotifierWatcher (org.kde.StatusNotifierWatcher)
on the session bus, to test tray icons on Linux without a desktop:

    python3 scripts/mock-sni-watcher.py &
    cargo run -p counter

Logs each item that registers, and each that leaves the bus, on stdout.
Needs python3-dbus and python3-gi (docker/linux-dev.Dockerfile).
"""
import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

NAME = "org.kde.StatusNotifierWatcher"
PROPS = "org.freedesktop.DBus.Properties"


def log(*args):
    print("[mock-sni-watcher]", *args, flush=True)


class Watcher(dbus.service.Object):
    def __init__(self, bus):
        super().__init__(bus, "/StatusNotifierWatcher")
        self.items = []
        bus.add_signal_receiver(
            self.owner_changed,
            signal_name="NameOwnerChanged",
            dbus_interface="org.freedesktop.DBus",
            path="/org/freedesktop/DBus",
        )

    @dbus.service.method(NAME, in_signature="s", sender_keyword="sender")
    def RegisterStatusNotifierItem(self, service, sender):
        # A bus name, or an object path on the caller's connection.
        item = sender + service if service.startswith("/") else str(service)
        if item not in self.items:
            self.items.append(item)
        log("registered", item, "from", sender)
        self.StatusNotifierItemRegistered(item)

    @dbus.service.method(NAME, in_signature="s")
    def RegisterStatusNotifierHost(self, service):
        pass

    @dbus.service.signal(NAME, signature="s")
    def StatusNotifierItemRegistered(self, item):
        pass

    @dbus.service.signal(NAME, signature="s")
    def StatusNotifierItemUnregistered(self, item):
        pass

    def properties(self):
        return {
            "RegisteredStatusNotifierItems": dbus.Array(self.items, signature="s"),
            "IsStatusNotifierHostRegistered": dbus.Boolean(True),
            "ProtocolVersion": dbus.Int32(0),
        }

    @dbus.service.method(PROPS, in_signature="ss", out_signature="v")
    def Get(self, interface, name):
        return self.properties()[name]

    @dbus.service.method(PROPS, in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        return self.properties()

    def owner_changed(self, name, old, new):
        if new:
            return
        for item in [i for i in self.items if i == name or i.startswith(name + "/")]:
            self.items.remove(item)
            log("unregistered", item)
            self.StatusNotifierItemUnregistered(item)


dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
watcher = Watcher(bus)
# Kept: the name is released when this is collected.
name = dbus.service.BusName(NAME, bus)
log("running")
GLib.MainLoop().run()
