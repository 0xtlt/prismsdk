package dev.prism;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;

/**
 * Lends the app's own files to other apps (the share sheet) through
 * `content://<package>.prism.files/<path>`, read-only, like AndroidX's
 * FileProvider without the dependency. Only files under the app's cache and
 * files directories are served.
 */
public final class PrismFiles extends ContentProvider {
    static String authority(Context c) {
        return c.getPackageName() + ".prism.files";
    }

    static Uri uriFor(Context c, File f) {
        return new Uri.Builder().scheme("content").authority(authority(c)).path(f.getAbsolutePath()).build();
    }

    private File file(Uri uri) throws FileNotFoundException {
        File f;
        try {
            f = new File(uri.getPath()).getCanonicalFile();
        } catch (IOException e) {
            throw new FileNotFoundException(uri.toString());
        }
        Context c = getContext();
        for (File allowed : new File[]{c.getCacheDir(), c.getFilesDir()}) {
            try {
                if (f.getPath().startsWith(allowed.getCanonicalPath() + File.separator)) return f;
            } catch (IOException ignored) {
            }
        }
        throw new FileNotFoundException(uri.toString());
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        return ParcelFileDescriptor.open(file(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public String getType(Uri uri) {
        String ext = MimeTypeMap.getFileExtensionFromUrl(uri.toString());
        String mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext);
        return mime != null ? mime : "application/octet-stream";
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        File f;
        try {
            f = file(uri);
        } catch (FileNotFoundException e) {
            return null;
        }
        MatrixCursor c = new MatrixCursor(new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE});
        c.addRow(new Object[]{f.getName(), f.length()});
        return c;
    }

    @Override public boolean onCreate() { return true; }
    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String s, String[] a) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues v, String s, String[] a) { throw new UnsupportedOperationException(); }
}
