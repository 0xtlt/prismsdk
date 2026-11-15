package dev.prism;

import android.app.backup.BackupAgent;
import android.app.backup.BackupDataInput;
import android.app.backup.BackupDataOutput;
import android.os.ParcelFileDescriptor;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;

/**
 * {@code cx.cloud_storage()} on Android: one file per key in {@code files/cloud}, backed up
 * key by key with the Android Backup Service (key/value backup) and restored on a new
 * device or a reinstall. Rust calls {@code BackupManager.dataChanged()} after each change
 * (PrismHost.cloudChanged); the system decides when to back up. Not a live sync.
 *
 * Only this directory is backed up: secrets ({@code files/secure}) are sealed by Keystore
 * keys that never leave the device.
 */
public class PrismBackup extends BackupAgent {
    private File dir() {
        File d = new File(getFilesDir(), "cloud");
        d.mkdirs();
        return d;
    }

    /** What was backed up last time: name → a stamp of its modification time and size. */
    private static Map<String, Long> read(ParcelFileDescriptor state) {
        Map<String, Long> out = new HashMap<>();
        if (state == null) return out;
        try (DataInputStream in = new DataInputStream(new FileInputStream(state.getFileDescriptor()))) {
            int n = in.readInt();
            for (int i = 0; i < n; i++) out.put(in.readUTF(), in.readLong());
        } catch (IOException ignored) {
            // First backup: everything goes.
        }
        return out;
    }

    private static void write(ParcelFileDescriptor state, Map<String, Long> files) throws IOException {
        DataOutputStream out = new DataOutputStream(new FileOutputStream(state.getFileDescriptor()));
        out.writeInt(files.size());
        for (Map.Entry<String, Long> e : files.entrySet()) {
            out.writeUTF(e.getKey());
            out.writeLong(e.getValue());
        }
        out.flush();
    }

    private Map<String, Long> current() {
        Map<String, Long> now = new HashMap<>();
        File[] files = dir().listFiles();
        if (files != null) {
            for (File f : files) {
                if (f.isFile() && !f.getName().endsWith(".tmp~")) {
                    now.put(f.getName(), f.lastModified() ^ (f.length() << 40));
                }
            }
        }
        return now;
    }

    @Override
    public void onBackup(ParcelFileDescriptor oldState, BackupDataOutput data, ParcelFileDescriptor newState) throws IOException {
        Map<String, Long> before = read(oldState), now = current();
        for (Map.Entry<String, Long> e : now.entrySet()) {
            if (e.getValue().equals(before.get(e.getKey()))) continue;
            byte[] bytes = Files.readAllBytes(new File(dir(), e.getKey()).toPath());
            data.writeEntityHeader(e.getKey(), bytes.length);
            data.writeEntityData(bytes, bytes.length);
        }
        for (String gone : before.keySet()) {
            if (!now.containsKey(gone)) data.writeEntityHeader(gone, -1);
        }
        write(newState, now);
    }

    @Override
    public void onRestore(BackupDataInput data, int appVersionCode, ParcelFileDescriptor newState) throws IOException {
        while (data.readNextHeader()) {
            String key = data.getKey();
            int size = data.getDataSize();
            if (key.contains("/") || key.startsWith(".")) {
                data.skipEntityData();
                continue;
            }
            byte[] bytes = new byte[Math.max(size, 0)];
            if (size > 0) data.readEntityData(bytes, 0, size);
            File f = new File(dir(), key), tmp = new File(dir(), key + ".tmp~");
            try (FileOutputStream o = new FileOutputStream(tmp)) {
                o.write(bytes);
            }
            if (!tmp.renameTo(f)) throw new IOException("could not restore " + key);
        }
        write(newState, current());
    }
}
