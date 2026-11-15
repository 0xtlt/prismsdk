package dev.prism;

import android.app.KeyguardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyPermanentlyInvalidatedException;
import android.security.keystore.KeyProperties;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyFactory;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.PrivateKey;
import java.security.PublicKey;
import java.security.spec.MGF1ParameterSpec;
import java.security.spec.X509EncodedKeySpec;
import java.util.List;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.OAEPParameterSpec;
import javax.crypto.spec.PSource;
import javax.crypto.spec.SecretKeySpec;

/**
 * Secrets, the device owner and browser sign-in for the Rust app
 * (prism_core::security), with framework APIs only:
 * <ul>
 * <li>secrets: AES-256-GCM with a key that never leaves the Android Keystore; the
 *     ciphertext lives in the app's files ({@code files/secure}, not backed up).
 *     Biometric items: the value is sealed with a fresh AES key, wrapped with an
 *     RSA key that needs the owner (BiometricPrompt + CryptoObject) to unwrap;</li>
 * <li>owner checks: {@link BiometricPrompt} (API 28), device credential from API 30;</li>
 * <li>sign-in: a Custom Tab (the protocol's intent extra, no androidx) or the
 *     browser; the redirect comes back as a deep link (PrismLinkActivity).</li>
 * </ul>
 */
final class PrismSecurity {
    private PrismSecurity() {}

    private static final String STORE = "AndroidKeyStore";
    private static final String AES_ALIAS = "prism.secure";
    private static final String RSA_ALIAS = "prism.secure.owner";
    private static final OAEPParameterSpec OAEP = new OAEPParameterSpec(
            "SHA-256", "MGF1", MGF1ParameterSpec.SHA1, PSource.PSpecified.DEFAULT);

    static Context ctx() { return PrismHost.context; }

    static void call(long id, String call, JSONObject c) {
        switch (call) {
            case "secure_get": get(id, c.optString("key"), c.optString("prompt", "")); break;
            case "secure_set": set(id, c.optString("key"), c.optString("value"), c.optBoolean("biometric")); break;
            case "secure_delete": file(c.optString("key")).delete(); PrismDevice.reply(id, null, null); break;
            case "biometry": PrismDevice.reply(id, "fields", status()); break;
            case "authenticate": authenticate(id, c.optString("reason"), c.optBoolean("passcode", true)); break;
            case "auth_redirect": PrismDevice.reply(id, "text", redirect(c.optString("preferred", ""))); break;
            case "web_auth": webAuth(id, c.optString("url"), c.optString("redirect")); break;
            // Sign in with Apple is Apple's (use prism::oauth with Apple's web endpoints).
            default: PrismDevice.fail(id, "unavailable", false);
        }
    }

    // --- Secrets ---------------------------------------------------------------------------

    static File file(String key) {
        File dir = new File(ctx().getFilesDir(), "secure");
        dir.mkdirs();
        try {
            byte[] h = MessageDigest.getInstance("SHA-256").digest(key.getBytes(StandardCharsets.UTF_8));
            StringBuilder name = new StringBuilder();
            for (byte b : h) name.append(String.format("%02x", b));
            return new File(dir, name.toString());
        } catch (Exception e) {
            return new File(dir, Integer.toHexString(key.hashCode()));
        }
    }

    private static KeyStore keystore() throws Exception {
        KeyStore ks = KeyStore.getInstance(STORE);
        ks.load(null);
        return ks;
    }

    private static SecretKey aesKey() throws Exception {
        KeyStore ks = keystore();
        if (ks.containsAlias(AES_ALIAS)) return (SecretKey) ks.getKey(AES_ALIAS, null);
        KeyGenerator g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE);
        g.init(new KeyGenParameterSpec.Builder(AES_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build());
        return g.generateKey();
    }

    /** The owner's RSA key pair: unwrapping needs biometrics (or the device credential, API 30+). */
    private static PublicKey ownerPublicKey() throws Exception {
        KeyStore ks = keystore();
        if (!ks.containsAlias(RSA_ALIAS)) {
            KeyGenParameterSpec.Builder b = new KeyGenParameterSpec.Builder(RSA_ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                    .setKeySize(2048)
                    .setDigests(KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_SHA1)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_RSA_OAEP)
                    .setUserAuthenticationRequired(true)
                    // New fingers or faces make it unusable: store the secret again.
                    .setInvalidatedByBiometricEnrollment(true);
            if (Build.VERSION.SDK_INT >= 30) {
                b.setUserAuthenticationParameters(0,
                        KeyProperties.AUTH_BIOMETRIC_STRONG | KeyProperties.AUTH_DEVICE_CREDENTIAL);
            }
            KeyPairGenerator g = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_RSA, STORE);
            g.initialize(b.build());
            g.generateKeyPair();
        }
        // Public key operations need no authentication: use the software provider.
        PublicKey pub = ks.getCertificate(RSA_ALIAS).getPublicKey();
        return KeyFactory.getInstance(pub.getAlgorithm()).generatePublic(new X509EncodedKeySpec(pub.getEncoded()));
    }

    /** {@code [0][iv 12][AES-GCM]} or, biometric, {@code [1][len 2][RSA(aes key)][iv 12][AES-GCM]}. */
    static void set(long id, String key, String value, boolean biometric) {
        try {
            byte[] plain = value.getBytes(StandardCharsets.UTF_8);
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            Cipher aes = Cipher.getInstance("AES/GCM/NoPadding");
            if (biometric) {
                byte[] raw = new byte[32];
                new java.security.SecureRandom().nextBytes(raw);
                Cipher rsa = Cipher.getInstance("RSA/ECB/OAEPWithSHA-256AndMGF1Padding");
                rsa.init(Cipher.ENCRYPT_MODE, ownerPublicKey(), OAEP);
                byte[] wrapped = rsa.doFinal(raw);
                aes.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(raw, "AES"));
                out.write(1);
                out.write(wrapped.length >> 8);
                out.write(wrapped.length & 0xff);
                out.write(wrapped);
            } else {
                aes.init(Cipher.ENCRYPT_MODE, aesKey());
                out.write(0);
            }
            out.write(aes.getIV());
            out.write(aes.doFinal(plain));
            File f = file(key), tmp = new File(f.getPath() + ".tmp~");
            try (FileOutputStream o = new FileOutputStream(tmp)) { o.write(out.toByteArray()); }
            if (!tmp.renameTo(f)) throw new java.io.IOException("rename failed");
            PrismDevice.reply(id, null, null);
        } catch (IllegalStateException e) {
            // No secure lock screen: the Keystore refuses keys that need the owner.
            PrismDevice.fail(id, "unavailable", false);
        } catch (Exception e) {
            PrismDevice.fail(id, "keystore: " + e, false);
        }
    }

    static void get(long id, String key, String prompt) {
        File f = file(key);
        if (!f.exists()) { PrismDevice.reply(id, "text", JSONObject.NULL); return; }
        try {
            ByteBuffer b = ByteBuffer.wrap(Files.readAllBytes(f.toPath()));
            int kind = b.get();
            if (kind == 0) {
                byte[] iv = new byte[12];
                b.get(iv);
                byte[] ct = new byte[b.remaining()];
                b.get(ct);
                Cipher aes = Cipher.getInstance("AES/GCM/NoPadding");
                aes.init(Cipher.DECRYPT_MODE, aesKey(), new GCMParameterSpec(128, iv));
                PrismDevice.reply(id, "text", new String(aes.doFinal(ct), StandardCharsets.UTF_8));
                return;
            }
            int len = (b.get() & 0xff) << 8 | (b.get() & 0xff);
            byte[] wrapped = new byte[len];
            b.get(wrapped);
            byte[] iv = new byte[12];
            b.get(iv);
            byte[] ct = new byte[b.remaining()];
            b.get(ct);
            Cipher rsa = Cipher.getInstance("RSA/ECB/OAEPWithSHA-256AndMGF1Padding");
            try {
                rsa.init(Cipher.DECRYPT_MODE, (PrivateKey) keystore().getKey(RSA_ALIAS, null), OAEP);
            } catch (KeyPermanentlyInvalidatedException e) {
                f.delete();
                PrismDevice.fail(id, "the enrolled biometrics changed: store the secret again", false);
                return;
            }
            prompt(id, prompt.isEmpty() ? label() : prompt, true, new BiometricPrompt.CryptoObject(rsa), cipher -> {
                try {
                    byte[] raw = cipher.doFinal(wrapped);
                    Cipher aes = Cipher.getInstance("AES/GCM/NoPadding");
                    aes.init(Cipher.DECRYPT_MODE, new SecretKeySpec(raw, "AES"), new GCMParameterSpec(128, iv));
                    PrismDevice.reply(id, "text", new String(aes.doFinal(ct), StandardCharsets.UTF_8));
                } catch (Exception e) {
                    PrismDevice.fail(id, "keystore: " + e, false);
                }
            });
        } catch (Exception e) {
            PrismDevice.fail(id, "keystore: " + e, false);
        }
    }

    // --- The device owner ----------------------------------------------------------------

    static String label() {
        return String.valueOf(ctx().getApplicationInfo().loadLabel(ctx().getPackageManager()));
    }

    /** The sensor: PackageManager features (BiometricManager does not say which). */
    static String kind() {
        PackageManager pm = ctx().getPackageManager();
        if (pm.hasSystemFeature(PackageManager.FEATURE_FINGERPRINT)) return "fingerprint";
        if (pm.hasSystemFeature(PackageManager.FEATURE_FACE)) return "face";
        if (pm.hasSystemFeature(PackageManager.FEATURE_IRIS)) return "iris";
        return "none";
    }

    static boolean deviceSecure() {
        KeyguardManager k = (KeyguardManager) ctx().getSystemService(Context.KEYGUARD_SERVICE);
        return k != null && k.isDeviceSecure();
    }

    @SuppressWarnings("deprecation")
    static int canAuthenticate() {
        BiometricManager m = (BiometricManager) ctx().getSystemService(Context.BIOMETRIC_SERVICE);
        if (m == null) return BiometricManager.BIOMETRIC_ERROR_NO_HARDWARE;
        return Build.VERSION.SDK_INT >= 30
                ? m.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_WEAK)
                : m.canAuthenticate();
    }

    static JSONObject status() {
        JSONObject o = new JSONObject();
        try {
            o.put("kind", kind());
            o.put("enrolled", canAuthenticate() == BiometricManager.BIOMETRIC_SUCCESS);
            o.put("passcode", deviceSecure());
            o.put("locked_out", false);
        } catch (JSONException ignored) {
        }
        return o;
    }

    interface Unlocked { void run(Cipher cipher); }

    static String error(int code, CharSequence message) {
        switch (code) {
            case BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED:
            case BiometricPrompt.BIOMETRIC_ERROR_CANCELED:
                return "cancelled";
            case BiometricPrompt.BIOMETRIC_ERROR_NO_BIOMETRICS:
            case BiometricPrompt.BIOMETRIC_ERROR_HW_NOT_PRESENT:
            case BiometricPrompt.BIOMETRIC_ERROR_HW_UNAVAILABLE:
            case BiometricPrompt.BIOMETRIC_ERROR_NO_DEVICE_CREDENTIAL:
                return "unavailable";
            default:
                return String.valueOf(message);
        }
    }

    /**
     * Shows BiometricPrompt. {@code crypto}: unlock that cipher (strong biometrics, or the
     * device credential from API 30); otherwise any biometric, then the PIN if {@code passcode}.
     */
    @SuppressWarnings("deprecation")
    static void prompt(long id, String title, boolean passcode, BiometricPrompt.CryptoObject crypto, Unlocked done) {
        if (PrismHost.activity == null) { PrismDevice.fail(id, "no window to show the prompt", false); return; }
        PrismHost.main.post(() -> {
            BiometricPrompt.Builder b = new BiometricPrompt.Builder(PrismHost.activity).setTitle(title);
            boolean credential;
            if (Build.VERSION.SDK_INT >= 30) {
                int strong = crypto != null ? BiometricManager.Authenticators.BIOMETRIC_STRONG
                        : BiometricManager.Authenticators.BIOMETRIC_WEAK;
                credential = passcode && deviceSecure();
                b.setAllowedAuthenticators(credential ? strong | BiometricManager.Authenticators.DEVICE_CREDENTIAL : strong);
            } else {
                // API 29: the device credential cannot unlock a CryptoObject.
                credential = passcode && crypto == null && deviceSecure();
                b.setDeviceCredentialAllowed(credential);
            }
            if (!credential) {
                b.setNegativeButton(ctx().getString(android.R.string.cancel), PrismHost.activity.getMainExecutor(),
                        (dialog, which) -> PrismDevice.fail(id, "cancelled", false));
            }
            BiometricPrompt.AuthenticationCallback cb = new BiometricPrompt.AuthenticationCallback() {
                @Override public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult r) {
                    BiometricPrompt.CryptoObject c = r.getCryptoObject();
                    done.run(c != null ? c.getCipher() : null);
                }
                @Override public void onAuthenticationError(int code, CharSequence message) {
                    PrismDevice.fail(id, error(code, message), false);
                }
                // A finger or face that does not match: the prompt stays and says so.
                @Override public void onAuthenticationFailed() {}
            };
            try {
                BiometricPrompt p = b.build();
                if (crypto != null) p.authenticate(crypto, new CancellationSignal(), PrismHost.activity.getMainExecutor(), cb);
                else p.authenticate(new CancellationSignal(), PrismHost.activity.getMainExecutor(), cb);
            } catch (Exception e) {
                PrismDevice.fail(id, "unavailable", false);
            }
        });
    }

    static void authenticate(long id, String reason, boolean passcode) {
        int can = canAuthenticate();
        if (can != BiometricManager.BIOMETRIC_SUCCESS && !(passcode && deviceSecure())) {
            PrismDevice.fail(id, "unavailable", false);
            return;
        }
        prompt(id, reason.isEmpty() ? label() : reason, passcode, null, cipher -> PrismDevice.reply(id, null, null));
    }

    // --- Browser sign-in -----------------------------------------------------------------

    /** The app's deep-link scheme (manifest meta-data {@code prism.scheme}). */
    static String scheme() {
        try {
            android.content.pm.ActivityInfo info = ctx().getPackageManager().getActivityInfo(
                    new android.content.ComponentName(ctx(), PrismActivity.class), PackageManager.GET_META_DATA);
            if (info.metaData != null && info.metaData.getString("prism.scheme") != null) {
                return info.metaData.getString("prism.scheme");
            }
        } catch (PackageManager.NameNotFoundException ignored) {
        }
        return "prism-" + ctx().getPackageName();
    }

    /** {@code preferred} if this app receives it, else {@code <scheme>:/oauth}. */
    static String redirect(String preferred) {
        if (!preferred.isEmpty()) {
            Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse(preferred)).addCategory(Intent.CATEGORY_BROWSABLE);
            i.setPackage(ctx().getPackageName());
            if (!ctx().getPackageManager().queryIntentActivities(i, 0).isEmpty()) return preferred;
        }
        return scheme() + ":/oauth";
    }

    static long pending = -1;
    static String pendingRedirect;
    /** The browser was opened (a return without the redirect is a cancel). */
    static boolean away;

    /** A browser with Custom Tabs (Chrome, Firefox, Samsung Internet…), if any. */
    static boolean customTabs() {
        Intent service = new Intent("android.support.customtabs.action.CustomTabsService");
        List<ResolveInfo> found = ctx().getPackageManager().queryIntentServices(service, 0);
        return found != null && !found.isEmpty();
    }

    static void webAuth(long id, String url, String redirect) {
        if (pending >= 0) PrismDevice.fail(pending, "cancelled", false);
        Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE);
        if (customTabs()) {
            // The Custom Tabs protocol is this extra (what androidx.browser adds).
            Bundle extras = new Bundle();
            extras.putBinder("android.support.customtabs.extra.SESSION", null);
            i.putExtras(extras);
            i.putExtra("android.support.customtabs.extra.SHARE_MENU_ITEM", false);
        }
        try {
            if (PrismHost.activity != null) PrismHost.activity.startActivity(i);
            else ctx().startActivity(i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            pending = id;
            pendingRedirect = redirect;
            away = true;
        } catch (Exception e) {
            PrismDevice.fail(id, "unavailable", false);
        }
    }

    /** A deep link arrived: the sign-in's redirect? Then it is answered and not a navigation. */
    static boolean redirected(Intent intent) {
        if (pending < 0 || intent == null || intent.getData() == null) return false;
        String uri = intent.getData().toString();
        Uri want = Uri.parse(pendingRedirect);
        Uri got = intent.getData();
        if (!String.valueOf(want.getScheme()).equalsIgnoreCase(String.valueOf(got.getScheme()))
                || !String.valueOf(want.getPath()).equals(String.valueOf(got.getPath()))) {
            return false;
        }
        long id = pending;
        pending = -1;
        away = false;
        PrismDevice.reply(id, "text", uri);
        return true;
    }

    /** The Activity is back: without a redirect, the user closed the browser. */
    static void resumed() {
        if (pending < 0 || !away) return;
        long id = pending;
        PrismHost.main.postDelayed(() -> {
            if (pending == id) {
                pending = -1;
                away = false;
                PrismDevice.fail(id, "cancelled", false);
            }
        }, 1500);
    }
}
