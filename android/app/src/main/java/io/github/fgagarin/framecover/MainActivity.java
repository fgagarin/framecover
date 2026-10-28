package io.github.fgagarin.framecover;

import android.app.NativeActivity;
import android.content.ActivityNotFoundException;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.Environment;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import android.util.Log;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * The app is the Rust library (libframecover.so, Slint UI); this activity
 * only adds what needs intents or content URIs. Rust calls the public
 * methods through JNI (src/android.rs) and is called back through
 * {@link #nativeOnPicked}.
 */
public class MainActivity extends NativeActivity {
    private static final String TAG = "FrameCover";
    private static final int REQUEST_PICK = 1;
    /** Picked files are copied here: FFmpeg and the writers need real paths. */
    private static final String PICKED_DIR = "picked";
    private static final String GALLERY_DIR = Environment.DIRECTORY_MOVIES + "/FrameCover";

    static {
        // NativeActivity loads the library itself, but JNI only resolves
        // this class's native methods in libraries loaded through Java.
        System.loadLibrary("framecover");
    }

    /** The cached copy of the picked file, or null (cancelled or failed). */
    private static native void nativeOnPicked(String path);

    /** Gallery URI of each published file, by the saved file's path. */
    private final Map<String, Uri> published = new ConcurrentHashMap<>();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // Copies from the previous run are no longer open.
        deleteRecursively(new File(getCacheDir(), PICKED_DIR));
        super.onCreate(savedInstanceState);
    }

    /**
     * Back leaves the app like Home instead of destroying the activity, so
     * the open video and the chosen frame are still there on return.
     */
    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        moveTaskToBack(true);
    }

    /** Show the system document picker for {@code mime} ("video/*", "image/*"). */
    public void pickFile(String mime) {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT)
                    .addCategory(Intent.CATEGORY_OPENABLE)
                    .setType(mime);
            try {
                startActivityForResult(intent, REQUEST_PICK);
            } catch (ActivityNotFoundException e) {
                Log.e(TAG, "no document picker", e);
                nativeOnPicked(null);
            }
        });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_PICK) {
            return;
        }
        Uri uri = resultCode == RESULT_OK && data != null ? data.getData() : null;
        if (uri == null) {
            nativeOnPicked(null);
            return;
        }
        // Videos can be large: copy off the UI thread.
        new Thread(() -> {
            String path = null;
            try {
                path = copyToCache(uri);
            } catch (IOException | SecurityException e) {
                Log.e(TAG, "could not copy " + uri, e);
            }
            nativeOnPicked(path);
        }, "pick-copy").start();
    }

    private String copyToCache(Uri uri) throws IOException {
        File dir = new File(new File(getCacheDir(), PICKED_DIR), Long.toString(System.nanoTime()));
        if (!dir.mkdirs()) {
            throw new IOException("could not create " + dir);
        }
        File target = new File(dir, displayName(uri));
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream out = new FileOutputStream(target)) {
            if (in == null) {
                throw new IOException("no content for " + uri);
            }
            copy(in, out);
        }
        return target.getAbsolutePath();
    }

    /** The document's file name (it names the saved copy), made path-safe. */
    private String displayName(Uri uri) {
        String name = null;
        try (Cursor cursor = getContentResolver().query(
                uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                name = cursor.getString(0);
            }
        }
        if (name == null || name.isEmpty()) {
            name = "video";
        }
        return name.replace('/', '_').replace('\0', '_');
    }

    /**
     * Copy the saved video at {@code path} into the shared Movies/FrameCover
     * collection (called on a worker thread). Throws on failure.
     */
    public void publishVideo(String path) throws IOException {
        File file = new File(path);
        ContentResolver resolver = getContentResolver();
        ContentValues values = new ContentValues();
        values.put(MediaStore.Video.Media.DISPLAY_NAME, file.getName());
        values.put(MediaStore.Video.Media.MIME_TYPE, mimeType(file.getName()));
        values.put(MediaStore.Video.Media.RELATIVE_PATH, GALLERY_DIR);
        values.put(MediaStore.Video.Media.IS_PENDING, 1);
        Uri uri = resolver.insert(
                MediaStore.Video.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY), values);
        if (uri == null) {
            throw new IOException("the media store refused " + file.getName());
        }
        try (InputStream in = new FileInputStream(file);
             OutputStream out = resolver.openOutputStream(uri)) {
            if (out == null) {
                throw new IOException("cannot write " + uri);
            }
            copy(in, out);
        } catch (IOException e) {
            resolver.delete(uri, null, null);
            throw e;
        }
        values.clear();
        values.put(MediaStore.Video.Media.IS_PENDING, 0);
        resolver.update(uri, values, null, null);
        published.put(path, uri);
    }

    /**
     * Open the published copy of {@code path} in a video player; nothing
     * happens for a file that was not published (the picked original).
     */
    public void viewPublished(String path) {
        Uri uri = published.get(path);
        if (uri == null) {
            Log.i(TAG, path + " has not been published");
            return;
        }
        String mime = mimeType(path);
        runOnUiThread(() -> startActivitySafely(new Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, mime)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)));
    }

    public void openUrl(String url) {
        runOnUiThread(() -> startActivitySafely(new Intent(Intent.ACTION_VIEW, Uri.parse(url))));
    }

    private void startActivitySafely(Intent intent) {
        try {
            startActivity(intent);
        } catch (ActivityNotFoundException e) {
            Log.w(TAG, "nothing handles " + intent, e);
        }
    }

    private static String mimeType(String name) {
        String lower = name.toLowerCase(Locale.ROOT);
        if (lower.endsWith(".mov")) return "video/quicktime";
        if (lower.endsWith(".m4v")) return "video/x-m4v";
        if (lower.endsWith(".mkv")) return "video/x-matroska";
        if (lower.endsWith(".webm")) return "video/webm";
        if (lower.endsWith(".avi")) return "video/x-msvideo";
        if (lower.endsWith(".flv")) return "video/x-flv";
        return "video/mp4";
    }

    private static void copy(InputStream in, OutputStream out) throws IOException {
        byte[] buffer = new byte[1 << 16];
        int n;
        while ((n = in.read(buffer)) > 0) {
            out.write(buffer, 0, n);
        }
    }

    private static void deleteRecursively(File file) {
        File[] children = file.listFiles();
        if (children != null) {
            for (File child : children) {
                deleteRecursively(child);
            }
        }
        if (file.exists() && !file.delete()) {
            Log.w(TAG, "could not delete " + file);
        }
    }
}
