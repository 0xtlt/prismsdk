package dev.prism;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;

/**
 * Receives deep links ({@code <scheme>:/path}, sign-in redirects) and hands them to the
 * running PrismActivity: CLEAR_TOP closes a Custom Tab left above it, SINGLE_TOP delivers
 * the link to {@code onNewIntent} instead of starting a second instance.
 */
public class PrismLinkActivity extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Intent i = new Intent(this, PrismActivity.class).setData(getIntent().getData());
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        startActivity(i);
        finish();
    }
}
