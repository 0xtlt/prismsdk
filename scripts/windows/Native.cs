// Win32 calls shared by uia-dump.ps1 and capture.ps1 (Add-Type -Path; the
// C# 5 compiler of Windows PowerShell 5.1).
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace PrismWindows
{
    public static class Native
    {
        delegate bool EnumProc(IntPtr hwnd, IntPtr data);

        [StructLayout(LayoutKind.Sequential)]
        struct Rect { public int Left, Top, Right, Bottom; }

        [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr data);
        [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
        [DllImport("user32.dll")] static extern int GetWindowLong(IntPtr hwnd, int index);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int size);
        [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr value);
        [DllImport("user32.dll")] static extern IntPtr GetDesktopWindow();
        [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool IsIconic(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
        [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);

        // Coordinates in physical pixels whatever the display scaling (does
        // nothing when the process already chose).
        public static void UsePhysicalPixels()
        {
            SetProcessDpiAwarenessContext(new IntPtr(-4)); // PER_MONITOR_AWARE_V2
        }

        // False on the session-0 desktop of an SSH logon: it is never shown,
        // so every window on it counts as hidden.
        public static bool DesktopShown()
        {
            return IsWindowVisible(GetDesktopWindow());
        }

        // Top-level windows of these processes that were shown (WS_VISIBLE),
        // even on a desktop that is not (UI Automation's desktop element and
        // Process.MainWindowHandle list nothing there). Console windows, which
        // Windows reports as the console program's own, are left out.
        public static IntPtr[] ShownWindows(int[] pids)
        {
            var found = new List<IntPtr>();
            EnumWindows((hwnd, data) =>
            {
                uint pid;
                GetWindowThreadProcessId(hwnd, out pid);
                if (Array.IndexOf(pids, (int)pid) < 0 || (GetWindowLong(hwnd, -16) & 0x10000000) == 0) return true;
                var name = new StringBuilder(64);
                GetClassName(hwnd, name, name.Capacity);
                var cls = name.ToString();
                if (cls != "ConsoleWindowClass" && cls != "PseudoConsoleWindow") found.Add(hwnd);
                return true;
            }, IntPtr.Zero);
            return found.ToArray();
        }

        public static bool IsMinimized(IntPtr hwnd)
        {
            return IsIconic(hwnd);
        }

        // x, y, width, height.
        public static int[] Bounds(IntPtr hwnd)
        {
            Rect r;
            GetWindowRect(hwnd, out r);
            return new[] { r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top };
        }

        // Draws the window into hdc, covered or not (PW_RENDERFULLCONTENT).
        public static bool Draw(IntPtr hwnd, IntPtr hdc)
        {
            return PrintWindow(hwnd, hdc, 2);
        }
    }
}
