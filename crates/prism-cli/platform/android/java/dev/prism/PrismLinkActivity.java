package dev.prism;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;

/**
 * Receives deep links ({@code <scheme>:/path}, sign-in redirects) and hands them to the
 * running PrismActivity: CLEAR_TOP closes a Custom Tab left above it, SINGLE_TOP delivers
 * the link to {@code onNewIntent} instead of starting a second instance.
 *
 * <p>A link to the sign-in's redirect URI that does not answer it (any app can send one) stops
 * here: forwarded, it would close the Custom Tab where the user is signing in.
 */
public class PrismLinkActivity extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        if (PrismSecurity.refuses(getIntent().getData())) {
            finish();
            return;
        }
        Intent i = new Intent(this, PrismActivity.class).setData(getIntent().getData());
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        startActivity(i);
        finish();
    }
}
