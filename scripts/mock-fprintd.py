#!/usr/bin/env python3
"""A stand-in for fprintd (net.reactivated.Fprint) on the session bus, to test
`authenticate` on Linux without a fingerprint reader:

    python3 scripts/mock-fprintd.py [verify-match|verify-no-match] &
    PRISM_FPRINTD_BUS=session cargo run -p counter

One device with one enrolled finger; VerifyStart answers with the given result
after a second. Needs python3-dbus and python3-gi (docker/linux-dev.Dockerfile).
"""
import sys

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

RESULT = sys.argv[1] if len(sys.argv) > 1 else "verify-match"
DEVICE = "net.reactivated.Fprint.Device"


class Manager(dbus.service.Object):
    @dbus.service.method("net.reactivated.Fprint.Manager", out_signature="o")
    def GetDefaultDevice(self):
        return "/net/reactivated/Fprint/Device/0"


class Device(dbus.service.Object):
    claimed = False

    @dbus.service.method(DEVICE, in_signature="s", out_signature="as")
    def ListEnrolledFingers(self, user):
        return ["right-index-finger"]

    @dbus.service.method(DEVICE, in_signature="s")
    def Claim(self, user):
        print("[mock-fprintd] claimed", flush=True)
        self.claimed = True

    @dbus.service.method(DEVICE)
    def Release(self):
        print("[mock-fprintd] released", flush=True)
        self.claimed = False

    @dbus.service.method(DEVICE, in_signature="s")
    def VerifyStart(self, finger):
        if not self.claimed:
            raise dbus.exceptions.DBusException("not claimed", name="net.reactivated.Fprint.Error.ClaimDevice")
        print(f"[mock-fprintd] verifying ({finger}) -> {RESULT}", flush=True)
        GLib.timeout_add(1000, lambda: self.VerifyStatus(RESULT, True) and False)

    @dbus.service.method(DEVICE)
    def VerifyStop(self):
        print("[mock-fprintd] verify stopped", flush=True)

    @dbus.service.signal(DEVICE, signature="sb")
    def VerifyStatus(self, result, done):
        pass


dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName("net.reactivated.Fprint", bus)
Manager(bus, "/net/reactivated/Fprint/Manager")
Device(bus, "/net/reactivated/Fprint/Device/0")
print("[mock-fprintd] ready", flush=True)
GLib.MainLoop().run()
