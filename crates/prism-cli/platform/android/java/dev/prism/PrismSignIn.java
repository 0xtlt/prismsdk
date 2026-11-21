package dev.prism;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/**
 * Whether a deep link answers the browser sign-in in progress: the rules of Rust's
 * {@code prism_core::security::is_sign_in_callback}. No Android classes, so prism-cli's tests
 * check it against the Rust function on the JVM.
 */
final class PrismSignIn {
    private PrismSignIn() {}

    /**
     * {@code callback} is for {@code redirect} (see {@link #sameTarget}) and, when the authorization
     * URL {@code url} carries a {@code state}, carries that same state, once.
     */
    static boolean answers(String url, String redirect, String callback) {
        if (!sameTarget(redirect, callback)) return false;
        String[] sent = parts(url), got = parts(callback);
        if (sent == null || got == null) return false;
        List<String> want = values(sent[3], "state");
        if (want.isEmpty()) return true;
        List<String> states = values(got[3], "state");
        return states.size() == 1 && states.get(0).equals(want.get(0));
    }

    /** {@code callback} is for {@code redirect}: its scheme, host, port and path, whatever its parameters. */
    static boolean sameTarget(String redirect, String callback) {
        String[] want = parts(redirect), got = parts(callback);
        return want != null && got != null
                && want[0].equals(got[0]) && Objects.equals(want[1], got[1]) && want[2].equals(got[2]);
    }

    /**
     * Lower-case scheme; lower-case authority (null without "//"); path ("/" when empty after an
     * authority); parameters (the query, or the fragment when there is no query). null without a
     * scheme.
     */
    static String[] parts(String url) {
        if (url == null) return null;
        int colon = url.indexOf(':');
        if (colon <= 0) return null;
        String scheme = url.substring(0, colon);
        for (int i = 0; i < scheme.length(); i++) {
            char c = scheme.charAt(i);
            boolean ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')
                    || c == '+' || c == '-' || c == '.';
            if (!ok) return null;
        }
        String rest = url.substring(colon + 1);
        String authority = null;
        if (rest.startsWith("//")) {
            rest = rest.substring(2);
            int end = indexOfAny(rest, "/?#");
            authority = lower(rest.substring(0, end));
            rest = rest.substring(end);
        }
        int end = indexOfAny(rest, "?#");
        String path = rest.substring(0, end);
        if (path.isEmpty() && authority != null) path = "/";
        String after = rest.substring(end);
        String params = "";
        if (after.startsWith("?")) {
            int hash = after.indexOf('#');
            params = hash < 0 ? after.substring(1) : after.substring(1, hash);
        } else if (after.startsWith("#")) {
            params = after.substring(1);
        }
        return new String[] {lower(scheme), authority, path, params};
    }

    /** The values of parameter {@code name} in {@code params} ({@code a=1&b=2}), decoded. */
    static List<String> values(String params, String name) {
        List<String> found = new ArrayList<>();
        for (String kv : params.split("&", -1)) {
            int eq = kv.indexOf('=');
            String k = eq < 0 ? kv : kv.substring(0, eq), v = eq < 0 ? "" : kv.substring(eq + 1);
            if (decode(k).equals(name)) found.add(decode(v));
        }
        return found;
    }

    /** A form-encoded value: {@code %XX} escapes, {@code +} for a space. */
    static String decode(String s) {
        byte[] b = s.getBytes(StandardCharsets.UTF_8);
        ByteArrayOutputStream out = new ByteArrayOutputStream(b.length);
        for (int i = 0; i < b.length; i++) {
            int hi = i + 2 < b.length ? hex(b[i + 1]) : -1, lo = i + 2 < b.length ? hex(b[i + 2]) : -1;
            if (b[i] == '+') {
                out.write(' ');
            } else if (b[i] == '%' && hi >= 0 && lo >= 0) {
                out.write(hi * 16 + lo);
                i += 2;
            } else {
                out.write(b[i]);
            }
        }
        return new String(out.toByteArray(), StandardCharsets.UTF_8);
    }

    private static int hex(byte c) {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        return -1;
    }

    private static int indexOfAny(String s, String chars) {
        for (int i = 0; i < s.length(); i++) {
            if (chars.indexOf(s.charAt(i)) >= 0) return i;
        }
        return s.length();
    }

    /** ASCII lower case (as Rust's {@code to_ascii_lowercase}). */
    private static String lower(String s) {
        StringBuilder out = new StringBuilder(s.length());
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            out.append(c >= 'A' && c <= 'Z' ? (char) (c + 32) : c);
        }
        return out.toString();
    }
}
