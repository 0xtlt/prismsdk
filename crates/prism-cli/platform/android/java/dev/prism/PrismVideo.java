package dev.prism;

import android.content.Context;
import android.net.Uri;
import android.view.Gravity;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.MediaController;
import android.widget.VideoView;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.HashMap;
import java.util.Map;

import org.json.JSONObject;

/**
 * Videos (#207): a {@link VideoView} with a {@link MediaController} (the
 * platform's controls). Remote sources get their headers
 * ({@code setVideoURI(uri, headers)}); app assets are copied to the cache
 * first. The video's size goes to Prism ({@code video_size}) once known, and
 * the end of the playback ({@code ended}).
 */
final class PrismVideo {
    private PrismVideo() {}

    static View create(PrismHost.Node n) {
        Context ctx = PrismHost.activity;
        FrameLayout box = new FrameLayout(ctx);
        box.setBackgroundColor(0xFF000000);
        VideoView v = new VideoView(ctx);
        box.addView(v, new FrameLayout.LayoutParams(
            FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT, Gravity.CENTER));
        MediaController controls = new MediaController(ctx);
        controls.setAnchorView(v);
        n.video = v;
        n.videoControls = controls;
        v.setMediaController(n.videoControlsOn ? controls : null);
        v.setOnPreparedListener(mp -> {
            mp.setLooping(n.videoLoop);
            mp.setVolume(n.videoMuted ? 0f : 1f, n.videoMuted ? 0f : 1f);
            int w = mp.getVideoWidth(), h = mp.getVideoHeight();
            if (w > 0 && h > 0) {
                JSONObject ev = PrismHost.event("video_size", n.id);
                PrismInput.put(ev, "w", w / PrismHost.density);
                PrismInput.put(ev, "h", h / PrismHost.density);
                PrismHost.send(ev);
            }
            if (n.videoAutoplay) v.start(); else v.seekTo(1);
        });
        v.setOnCompletionListener(mp -> PrismHost.send(PrismHost.event("ended", n.id)));
        return box;
    }

    /** The source: a URL, then one {@code name: value} header per line. */
    static void load(PrismHost.Node n, String text) {
        VideoView v = n.video;
        if (v == null || text.equals(n.videoSource)) return;
        n.videoSource = text;
        String[] lines = text.split("\n");
        String url = lines.length > 0 ? lines[0].trim() : "";
        if (url.isEmpty()) return;
        Map<String, String> headers = new HashMap<>();
        for (int i = 1; i < lines.length; i++) {
            int colon = lines[i].indexOf(':');
            if (colon > 0) headers.put(lines[i].substring(0, colon).trim(), lines[i].substring(colon + 1).trim());
        }
        if (url.startsWith("http://") || url.startsWith("https://")) {
            v.setVideoURI(Uri.parse(url), headers);
        } else if (url.startsWith("file://")) {
            v.setVideoURI(Uri.parse(url));
        } else if (url.startsWith("/")) {
            v.setVideoURI(Uri.fromFile(new File(url)));
        } else {
            File copy = asset(url);
            if (copy != null) v.setVideoURI(Uri.fromFile(copy));
        }
    }

    /** {@code controls autoplay loop muted poster=<url>} ({@code prism_core::VideoConfig}). */
    static void config(PrismHost.Node n, String config) {
        String flags = config.split("poster=")[0];
        java.util.List<String> t = java.util.Arrays.asList(flags.trim().split("\\s+"));
        n.videoControlsOn = t.contains("controls");
        n.videoAutoplay = t.contains("autoplay");
        n.videoLoop = t.contains("loop");
        n.videoMuted = t.contains("muted");
        if (n.video != null) n.video.setMediaController(n.videoControlsOn ? n.videoControls : null);
    }

    /** An app asset copied to the cache (a player needs a file). */
    private static File asset(String name) {
        if (name.contains("..")) return null;
        File dir = new File(PrismHost.activity.getCacheDir(), "media");
        File out = new File(dir, name.replace('/', '_'));
        if (out.exists()) return out;
        dir.mkdirs();
        try (InputStream in = PrismHost.activity.getAssets().open(name); OutputStream o = new FileOutputStream(out)) {
            byte[] buf = new byte[64 * 1024];
            for (int r; (r = in.read(buf)) > 0; ) o.write(buf, 0, r);
            return out;
        } catch (Exception e) {
            out.delete();
            return null;
        }
    }
}
