#!/usr/bin/env python3
"""The X input focus, through libX11 (no xdotool needed). On a bare X server
(Xvfb, no window manager) only the app moves it, when it brings a window
forward: scripts/linux-tray-smoke.sh checks a tray click with it.

  x11-focus.py clear     focus nowhere (None)
  x11-focus.py get       prints the focused window's id and name
                         ("0x0 None" when nowhere)
"""

import ctypes
import sys

x11 = ctypes.cdll.LoadLibrary("libX11.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XGetInputFocus.argtypes = [
    ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_ulong),
    ctypes.POINTER(ctypes.c_int),
]
x11.XFetchName.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_char_p)]
x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]

NONE, POINTER_ROOT, REVERT_TO_NONE, CURRENT_TIME = 0, 1, 0, 0

display = x11.XOpenDisplay(None)
if not display:
    sys.exit("x11-focus: no X display (DISPLAY unset?)")

if sys.argv[1:] == ["clear"]:
    x11.XSetInputFocus(display, NONE, REVERT_TO_NONE, CURRENT_TIME)
    x11.XSync(display, 0)
elif sys.argv[1:] == ["get"]:
    window, revert = ctypes.c_ulong(), ctypes.c_int()
    x11.XGetInputFocus(display, ctypes.byref(window), ctypes.byref(revert))
    name = "None"
    if window.value == POINTER_ROOT:
        name = "PointerRoot"
    elif window.value != NONE:
        title = ctypes.c_char_p()
        if x11.XFetchName(display, window.value, ctypes.byref(title)) and title.value:
            name = title.value.decode("utf-8", "replace")
        else:
            name = "(untitled)"
    print(f"{window.value:#x} {name}")
else:
    sys.exit(__doc__)
