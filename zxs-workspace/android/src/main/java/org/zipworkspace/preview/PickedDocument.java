package org.zipworkspace.preview;

import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.content.pm.ProviderInfo;
import android.database.Cursor;
import android.net.Uri;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.provider.DocumentsContract;
import android.provider.DocumentsContract.Document;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import java.io.FileDescriptor;
import java.io.IOException;
import java.util.Locale;
import java.util.function.LongConsumer;

/** One user-selected document from a platform-signed AOSP-compatible primary storage provider. */
final class PickedDocument implements AutoCloseable {
    static final String AUTH = "com.android.externalstorage.documents";
    private static final String PACKAGE = "com.android.externalstorage";
    private static final String MANAGE = "android.permission.MANAGE_DOCUMENTS";
    final String title;
    final ArchiveIndex index;
    private final Uri uri;
    private final StructStat observed;
    private ParcelFileDescriptor held;
    private PickedDocument(String title, ArchiveIndex index, Uri uri, StructStat observed, ParcelFileDescriptor held) {
        this.title = title; this.index = index; this.uri = uri; this.observed = observed; this.held = held;
    }
    private synchronized ParcelFileDescriptor borrow() throws IOException {
        ArchiveIndex.require(held != null);
        return ParcelFileDescriptor.dup(held.getFileDescriptor()); // same open file description, not a URI reopen
    }
    @Override public synchronized void close() {
        if (held == null) return;
        ParcelFileDescriptor old = held; held = null;
        try { old.close(); } catch (IOException ignored) { /* no retry of a closed descriptor number */ }
    }
    StoredMember.Result checkMember(Context ctx, ArchiveIndex.Item item, CancellationSignal cancel,
                                    StoredMember.Mode mode, LongConsumer progress) throws IOException {
        cancel.throwIfCanceled();
        try (ParcelFileDescriptor lease = borrow()) {
            Reader reader = new Reader(ctx, uri, lease.getFileDescriptor(), observed, cancel, progress);
            reader.validate();
            StoredMember.Result result = index.checkMember(reader, item, mode);
            reader.validate();
            return result;
        }
    }

    boolean isManagedSnapshotName() { return title.toLowerCase(Locale.ROOT).endsWith(".zxs"); }
    long fileBytes() { return observed.st_size; }

    String rawDigest(Context ctx, CancellationSignal cancel, LongConsumer progress) throws IOException {
        cancel.throwIfCanceled();
        try (ParcelFileDescriptor lease = borrow()) {
            Reader reader = new Reader(ctx, uri, lease.getFileDescriptor(), observed, cancel, null);
            reader.validate();
            String digest = ArchiveIndex.rawDigest(reader, fileBytes(), progress);
            reader.checkEnd();
            reader.validate();
            return digest;
        }
    }
    ArchiveIndex.SnapshotMarker verifySnapshot(Context ctx, CancellationSignal cancel, LongConsumer progress)
            throws IOException {
        ArchiveIndex.require(isManagedSnapshotName());
        cancel.throwIfCanceled();
        try (ParcelFileDescriptor lease = borrow()) {
            Reader reader = new Reader(ctx, uri, lease.getFileDescriptor(), observed, cancel, progress);
            reader.validate();
            ArchiveIndex.SnapshotMarker result = index.verifySnapshot(reader);
            reader.validate();
            return result;
        }
    }

    String textPage(Context ctx, ArchiveIndex.Item item, ArchiveIndex.TextIndex proof, int page, CancellationSignal cancel)
            throws IOException {
        cancel.throwIfCanceled();
        try (ParcelFileDescriptor lease = borrow()) {
            Reader reader = new Reader(ctx, uri, lease.getFileDescriptor(), observed, cancel, null);
            reader.validate();
            String result = index.textPage(reader, item, proof, page);
            reader.validate();
            return result;
        }
    }

    /** Final result drawing rechecks the existing grant and original FD, never a new URI. */
    synchronized void checkPresentation(Context ctx) throws IOException {
        ArchiveIndex.require(held != null);
        checkObserved(ctx, uri, held.getFileDescriptor(), observed);
    }
    private static void checkObserved(Context ctx, Uri uri, FileDescriptor fd, StructStat original) throws IOException {
        ArchiveIndex.require(ctx.checkUriPermission(uri, Process.myPid(), Process.myUid(),
                Intent.FLAG_GRANT_READ_URI_PERMISSION) == PackageManager.PERMISSION_GRANTED);
        try { ArchiveIndex.require(same(original, Os.fstat(fd))); }
        catch (ErrnoException ex) { throw new IOException("Selected file no longer available", ex); }
    }

    static Intent picker() {
        return new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                .setType("*/*").putExtra(Intent.EXTRA_LOCAL_ONLY, true)
                .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, false)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
    }
    static Uri single(Intent result) throws IOException {
        ArchiveIndex.require(result != null && (result.getFlags() & Intent.FLAG_GRANT_READ_URI_PERMISSION) != 0);
        Uri u = result.getData(); ClipData clip = result.getClipData();
        ArchiveIndex.require(u != null && (clip == null || (clip.getItemCount() == 1
                && u.equals(clip.getItemAt(0).getUri()))));
        checkUri(u);
        return u;
    }
    private static void checkUri(Uri u) throws IOException {
        ArchiveIndex.require(u != null && "content".equals(u.getScheme()) && AUTH.equals(u.getEncodedAuthority())
                && u.getEncodedQuery() == null && u.getEncodedFragment() == null
                && u.getPathSegments().size() == 2 && "document".equals(u.getPathSegments().get(0)));
        String id = DocumentsContract.getDocumentId(u);
        // This convention is specific to the one admitted provider. Never turn id into a filesystem path.
        ArchiveIndex.require(id.length() > 8 && id.length() <= 4096 && id.startsWith("primary:")
                && DocumentsContract.buildDocumentUri(AUTH, id).equals(u));
    }
    private static boolean same(StructStat a, StructStat b) {
        return a.st_dev == b.st_dev && a.st_ino == b.st_ino && a.st_mode == b.st_mode
                && a.st_size == b.st_size && a.st_nlink == b.st_nlink
                && a.st_mtim.tv_sec == b.st_mtim.tv_sec && a.st_mtim.tv_nsec == b.st_mtim.tv_nsec
                && a.st_ctim.tv_sec == b.st_ctim.tv_sec && a.st_ctim.tv_nsec == b.st_ctim.tv_nsec;
    }
    static PickedDocument inspect(Context ctx, Uri uri, CancellationSignal cancel) throws IOException {
        checkUri(uri); cancel.throwIfCanceled();
        PackageManager pm = ctx.getPackageManager();
        ProviderInfo info = pm.resolveContentProvider(AUTH, 0);
        ArchiveIndex.require(info != null && info.applicationInfo != null && info.enabled && info.exported
                && PACKAGE.equals(info.packageName) && AUTH.equals(info.authority)
                && (info.applicationInfo.flags & ApplicationInfo.FLAG_SYSTEM) != 0
                && pm.checkSignatures("android", PACKAGE) == PackageManager.SIGNATURE_MATCH
                && MANAGE.equals(info.readPermission) && MANAGE.equals(info.writePermission)
                && DocumentsContract.isDocumentUri(ctx, uri)
                && ctx.checkUriPermission(uri, Process.myPid(), Process.myUid(), Intent.FLAG_GRANT_READ_URI_PERMISSION)
                         == PackageManager.PERMISSION_GRANTED);
        String id = DocumentsContract.getDocumentId(uri), title;
        long size;
        String[] columns = {Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME,
                Document.COLUMN_MIME_TYPE, Document.COLUMN_FLAGS, Document.COLUMN_SIZE};
        ContentResolver resolver = ctx.getContentResolver();
        try (Cursor c = resolver.query(uri, columns, null, null, null, cancel)) {
            ArchiveIndex.require(c != null && c.moveToFirst() && id.equals(c.getString(0))
                    && !c.isNull(1) && !c.isNull(2) && !Document.MIME_TYPE_DIR.equals(c.getString(2))
                    && c.getType(3) == Cursor.FIELD_TYPE_INTEGER
                    && (c.getLong(3) & (Document.FLAG_VIRTUAL_DOCUMENT | Document.FLAG_PARTIAL)) == 0
                    && c.getType(4) == Cursor.FIELD_TYPE_INTEGER);
            title = c.getString(1); size = c.getLong(4);
            String ext = title.toLowerCase(Locale.ROOT);
            ArchiveIndex.require(ArchiveIndex.safeComponent(title) && (ext.endsWith(".zxs") || ext.endsWith(".zip"))
                    && size >= 22 && size <= ArchiveIndex.MAX_ARCHIVE && !c.moveToNext());
        }
        cancel.throwIfCanceled();
        ParcelFileDescriptor pfd = resolver.openFileDescriptor(uri, "r", cancel);
        ArchiveIndex.require(pfd != null);
        boolean transferred = false;
        try {
            FileDescriptor fd = pfd.getFileDescriptor();
            StructStat original = Os.fstat(fd);
            ArchiveIndex.require(OsConstants.S_ISREG(original.st_mode) && original.st_ino != 0
                    && original.st_dev != 0 && original.st_nlink > 0 && original.st_size == size);
            Os.lseek(fd, 0, OsConstants.SEEK_CUR);
            Reader reader = new Reader(ctx, uri, fd, original, cancel, null);
            reader.validate();
            ArchiveIndex index = ArchiveIndex.read(reader, size);
            reader.validate();
            PickedDocument result = new PickedDocument(title, index, uri, original, pfd);
            transferred = true;
            return result;
        } catch (ErrnoException ex) { throw new IOException("Cannot inspect this selected file", ex); }
        finally { if (!transferred) pfd.close(); }
    }

    private static final class Reader implements ArchiveIndex.MemberSource {
        private final Context ctx;
        private final Uri uri;
        private final FileDescriptor fd;
        private final StructStat original;
        private final CancellationSignal cancel;
        private final LongConsumer progress;
        private long untilCheck;
        Reader(Context ctx, Uri uri, FileDescriptor fd, StructStat original, CancellationSignal cancel,
               LongConsumer progress) {
            this.ctx = ctx; this.uri = uri; this.fd = fd; this.original = original; this.cancel = cancel;
            this.progress = progress;
        }
        public void check() { cancel.throwIfCanceled(); }
        public void bodyProcessed(int length) {
            if (progress != null) progress.accept(length);
        }
        void validate() throws IOException {
            check();
            checkObserved(ctx, uri, fd, original);
            untilCheck = 8L << 20;
        }
        void checkEnd() throws IOException {
            byte[] trailing = new byte[1];
            for (;;) {
                check();
                try {
                    ArchiveIndex.require(Os.pread(fd, trailing, 0, 1, original.st_size) == 0);
                    return;
                } catch (ErrnoException ex) {
                    if (ex.errno != OsConstants.EINTR) throw new IOException("Could not confirm the original file end", ex);
                }
            }
        }
        public byte[] read(long off, int len) throws IOException {
            ArchiveIndex.require(len >= 0 && len <= ArchiveIndex.MAX_CD);
            byte[] b = new byte[len]; fill(off, b, len); return b;
        }
        public void fill(long off, byte[] b, int len) throws IOException {
            check();
            ArchiveIndex.require(off >= 0 && len >= 0 && len <= b.length && off <= original.st_size - len);
            for (int n = 0; n < len;) {
                check();
                if (untilCheck <= 0) validate();
                try {
                    int k = Os.pread(fd, b, n, Math.min(65536, len - n), off + n);
                    ArchiveIndex.require(k > 0 && k <= len - n);
                    n += k; untilCheck -= k;
                } catch (ErrnoException ex) {
                    if (ex.errno != OsConstants.EINTR) throw new IOException("Read unavailable", ex);
                }
            }
        }
    }
}
