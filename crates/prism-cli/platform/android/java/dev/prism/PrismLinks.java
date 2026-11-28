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

    /** {@link #appScheme}: not under the app schemes' domain, the WebView loads it. */
    static final int NOT_APP = 0;
    /** {@link #appScheme}: an app scheme's URL, the app's handler answers it. */
    static final int SERVE = 1;
    /** {@link #appScheme}: under the domain but not as the app serves it: 404, never the network. */
    static final int REFUSE = 2;

    /**
     * What a request to {@code scheme://host} is (#343): app schemes are served at
     * {@code https://<host>.<scheme>.prism.invalid} ({@code prism_core::web_view::engine_url}).
     * Any other form of a host under {@code prism.invalid} (another scheme such as {@code http},
     * a trailing dot, the bare domain) is refused, so it never reaches DNS.
     */
    static int appScheme(String scheme, String host) {
        if (host == null) return NOT_APP;
        String lower = host.toLowerCase(java.util.Locale.ROOT);
        String bare = lower;
        while (bare.endsWith(".")) bare = bare.substring(0, bare.length() - 1);
        if (!bare.equals("prism.invalid") && !bare.endsWith(".prism.invalid")) return NOT_APP;
        boolean canonical = "https".equalsIgnoreCase(scheme) && bare.equals(lower)
                && bare.endsWith(".prism.invalid");
        return canonical ? SERVE : REFUSE;
    }
}
