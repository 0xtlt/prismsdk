package dev.prism;

import java.util.Set;

/**
 * What a web view may open outside the app: no Android classes, so prism-cli's tests run it on
 * the JVM. As on Apple platforms and Windows: only what the user asked for, and only web links,
 * mail and phone numbers, never another app's own scheme ({@code intent:}, {@code market:}, a
 * custom one).
 */
final class PrismLinks {
    private PrismLinks() {}

    /**
     * A new window ({@code target="_blank"}, {@code window.open}) opens its first URL outside the
     * app only when the user asked for it (a tap, not a script on its own) and the sandbox
     * ({@code null}: none) allows popups.
     */
    static boolean popup(boolean gesture, Set<String> sandbox) {
        return gesture && (sandbox == null || sandbox.contains("popups"));
    }

    /** {@code url} may open in another app: an http, https, mailto or tel URL. */
    static boolean external(String url) {
        if (url == null) return false;
        int colon = url.indexOf(':');
        if (colon <= 0) return false;
        StringBuilder scheme = new StringBuilder(colon);
        for (int i = 0; i < colon; i++) {
            char c = url.charAt(i);
            if (c >= 'A' && c <= 'Z') c = (char) (c + 32);
            scheme.append(c);
        }
        switch (scheme.toString()) {
            case "http":
            case "https":
            case "mailto":
            case "tel":
                return true;
            default:
                return false;
        }
    }
}
