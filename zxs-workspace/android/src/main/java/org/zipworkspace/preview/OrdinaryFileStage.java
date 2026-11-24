package org.zipworkspace.preview;

import android.content.Context;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.os.StatFs;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;

import java.io.File;
import java.io.FileDescriptor;
import java.io.IOException;
import java.io.InputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.UUID;

/** Freezes one complete selected body before any submission is possible. */
final class OrdinaryFileStage {
    private static final int BUFFER = 64 * 1024;
    private static final int CREATE = OsConstants.O_RDWR | OsConstants.O_CREAT | OsConstants.O_EXCL
            | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC;
    private static final String STORE = "ordinary-bodies";

    record Staged(String id, long length, String sha256) {}
    record Baseline(long length, String sha256) {}

    static final class Unchanged extends IOException {
        Unchanged() { super("The replacement has the same complete content as its original node"); }
    }

    static final class Unconfirmed extends IOException {
        private final transient Staged original;

        Unconfirmed(Staged original, Exception cause) {
            super("Could not confirm the staged body. Keep the same operation ID.", cause);
            this.original = original;
        }

        Staged original() { return original; }
    }

    private static final class Publication {
        Staged evidence;
    }

    interface Source {
        long size();
        void validate() throws IOException;
        void fill(long offset, byte[] bytes, int count) throws IOException;
        void checkEnd() throws IOException;
    }

    private OrdinaryFileStage() {}

    static String newId() { return UUID.randomUUID().toString().replace("-", ""); }

    static synchronized Staged stage(Context context, String id, Source source, CancellationSignal cancel)
            throws IOException {
        return stage(context, id, source, cancel, null);
    }

    static synchronized Staged stage(Context context, String id, Source source, CancellationSignal cancel,
                                     Baseline baseline) throws IOException {
        require(id != null && id.matches("[0-9a-f]{32}"));
        var path = new File(context.getNoBackupFilesDir(), STORE);
        int uid = Process.myUid();
        cancel.throwIfCanceled();
        require(uid != 0 && path.isAbsolute());
        var publication = new Publication();
        try (var parent = new LocalWorkspaceStore.Directory(path.getParentFile(), uid, false)) {
            parent.check();
            try {
                Os.mkdir(path.getPath(), 0700);
            } catch (ErrnoException exists) {
                if (exists.errno != OsConstants.EEXIST) throw exists;
            }
            try (var store = new LocalWorkspaceStore.Directory(path, uid, true)) {
                require(store.original.st_dev == parent.original.st_dev);
                parent.sync();
                long available = new StatFs(path.getPath()).getAvailableBytes();
                var result = publish(store, id, source, cancel, available, publication, baseline);
                parent.check();
                return result;
            }
        } catch (ErrnoException | IOException | RuntimeException failure) {
            if (publication.evidence != null) throw new Unconfirmed(publication.evidence, failure);
            if (failure instanceof ErrnoException errno) throw errno.rethrowAsIOException();
            if (failure instanceof IOException io) throw io;
            throw (RuntimeException) failure;
        }
    }

    // Reclaim only a recorded operation and only after proving its original body.
    // A lone .pending is never a body or an automatic resend candidate.
    static synchronized Staged reopen(Context context, Staged expected, CancellationSignal cancel)
            throws IOException {
        try (var verified = openVerified(context, expected, cancel)) {
            return expected;
        }
    }

    static InputStream openBody(Context context, Staged expected) throws IOException {
        return new ParcelFileDescriptor.AutoCloseInputStream(
                openVerified(context, expected, new CancellationSignal()));
    }

    static synchronized void discardUnbound(Context context, Staged expected) throws IOException {
        // The journal holds its process-wide lock and has confirmed no binding or attempt.
        try (var held = openVerified(context, expected, new CancellationSignal())) {
            var path = new File(context.getNoBackupFilesDir(), STORE);
            try (var store = new LocalWorkspaceStore.Directory(path, Process.myUid(), true);
                 var names = store.openNames()) {
                var body = new File(path, expected.id() + ".body");
                var original = Os.fstat(held.getFileDescriptor());
                store.check();
                checkFile(Os.lstat(body.getPath()), original, store, expected.length(), 1);
                names.deleteFile(body.toPath().getFileName());
                store.sync();
            } catch (ErrnoException failure) {
                throw failure.rethrowAsIOException();
            }
        }
    }

    private static synchronized ParcelFileDescriptor openVerified(Context context, Staged expected,
                                                                  CancellationSignal cancel)
            throws IOException {
        require(expected != null && expected.id() != null && expected.id().matches("[0-9a-f]{32}")
                && expected.length() >= 0 && expected.sha256() != null
                && expected.sha256().matches("[0-9a-f]{64}"));
        var path = new File(context.getNoBackupFilesDir(), STORE);
        int uid = Process.myUid();
        require(uid != 0 && path.isAbsolute());
        ParcelFileDescriptor verified = null;
        try (var parent = new LocalWorkspaceStore.Directory(path.getParentFile(), uid, false);
             var store = new LocalWorkspaceStore.Directory(path, uid, true);
             var names = store.openNames()) {
            require(store.original.st_dev == parent.original.st_dev);
            var body = new File(path, expected.id() + ".body");
            var pending = new File(path, expected.id() + ".pending");
            try (var in = new LocalWorkspaceStore.Handle(body, OsConstants.O_RDONLY | OsConstants.O_NONBLOCK
                    | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC, 0)) {
                var original = Os.fstat(in.fd);
                long links = original.st_nlink;
                require(links == 1 || links == 2);
                checkFile(Os.lstat(body.getPath()), original, store, expected.length(), links);
                verifyBytes(in.fd, new byte[BUFFER], expected.length(),
                        HexFormat.of().parseHex(expected.sha256()), cancel);
                cancel.throwIfCanceled();
                parent.check();
                store.check();
                var readback = Os.fstat(in.fd);
                checkFile(readback, original, store, expected.length(), links);
                require(readback.st_mtim.compareTo(original.st_mtim) == 0
                        && readback.st_ctim.compareTo(original.st_ctim) == 0);
                if (links == 2) {
                    checkFile(Os.lstat(pending.getPath()), original, store, expected.length(), 2);
                    names.deleteFile(pending.toPath().getFileName());
                }
                store.sync();
                checkFile(Os.lstat(body.getPath()), original, store, expected.length(), 1);
                checkFile(Os.fstat(in.fd), original, store, expected.length(), 1);
                verified = ParcelFileDescriptor.dup(in.fd);
            }
        } catch (ErrnoException | IOException | RuntimeException failure) {
            if (verified != null) {
                try { verified.close(); } catch (IOException close) { failure.addSuppressed(close); }
            }
            if (failure instanceof ErrnoException errno) throw errno.rethrowAsIOException();
            if (failure instanceof IOException io) throw io;
            throw (RuntimeException) failure;
        }
        return verified;
    }

    private static Staged publish(LocalWorkspaceStore.Directory store, String id, Source source,
                                  CancellationSignal cancel, long available, Publication publication, Baseline baseline)
            throws ErrnoException, IOException {
        long length = source.size();
        require(length >= 0 && length <= available);
        var pending = new File(store.path, id + ".pending");
        var body = new File(store.path, id + ".body");
        var bytes = new byte[BUFFER];
        var digest = sha();
        try (var names = store.openNames();
             var out = new LocalWorkspaceStore.Handle(pending, CREATE, 0600)) {
            var original = Os.fstat(out.fd);
            try {
                checkFile(original, original, store, 0, 1);
                source.validate();
                for (long offset = 0; offset < length;) {
                    cancel.throwIfCanceled();
                    int count = (int) Math.min(bytes.length, length - offset);
                    source.fill(offset, bytes, count);
                    digest.update(bytes, 0, count);
                    write(out.fd, bytes, count, cancel);
                    offset += count;
                }
                source.checkEnd();
                source.validate();
                byte[] expected = digest.digest();
                String sha256 = HexFormat.of().formatHex(expected);
                if (baseline != null && baseline.length() == length && baseline.sha256().equals(sha256)) {
                    throw new Unchanged();
                }
                Os.fsync(out.fd);
                checkFile(Os.fstat(out.fd), original, store, length, 1);
                verifyBytes(out.fd, bytes, length, expected, cancel);
                source.checkEnd();
                source.validate();
                cancel.throwIfCanceled();
                store.check();
                checkFile(Os.lstat(pending.getPath()), original, store, length, 1);
                var completed = new Staged(id, length, sha256);
                Os.link(pending.getPath(), body.getPath());
                publication.evidence = completed;
            } finally {
                // Delete only the candidate inode we still hold open.
                store.check();
                var named = Os.lstat(pending.getPath());
                require(LocalWorkspaceStore.same(named, original));
                names.deleteFile(pending.toPath().getFileName());
                store.sync();
            }
            checkFile(Os.lstat(body.getPath()), original, store, length, 1);
            checkFile(Os.fstat(out.fd), original, store, length, 1);
            return publication.evidence;
        }
    }

    private static void checkFile(StructStat seen, StructStat original, LocalWorkspaceStore.Directory store,
                                  long length, long links) throws IOException {
        require(LocalWorkspaceStore.same(seen, original) && seen.st_uid == store.uid
                && seen.st_dev == store.original.st_dev && OsConstants.S_ISREG(seen.st_mode)
                && (seen.st_mode & 07777) == 0600 && seen.st_nlink == links && seen.st_size == length);
    }

    private static void write(FileDescriptor fd, byte[] bytes, int count, CancellationSignal cancel)
            throws ErrnoException, IOException {
        for (int written = 0; written < count;) {
            cancel.throwIfCanceled();
            try {
                int n = Os.write(fd, bytes, written, count - written);
                require(n > 0 && n <= count - written);
                written += n;
            } catch (ErrnoException err) {
                if (err.errno != OsConstants.EINTR) throw err;
            }
        }
    }

    private static void verifyBytes(FileDescriptor fd, byte[] bytes, long length, byte[] expected,
                                    CancellationSignal cancel) throws ErrnoException, IOException {
        require(MessageDigest.isEqual(expected, expectedDigest(fd, bytes, length, cancel)));
    }

    private static byte[] expectedDigest(FileDescriptor fd, byte[] bytes, long length, CancellationSignal cancel)
            throws ErrnoException, IOException {
        var digest = sha();
        for (long offset = 0; offset < length;) {
            cancel.throwIfCanceled();
            require(!Thread.currentThread().isInterrupted());
            try {
                int count = (int) Math.min(bytes.length, length - offset);
                int n = Os.pread(fd, bytes, 0, count, offset);
                require(n > 0 && n <= count);
                digest.update(bytes, 0, n);
                offset += n;
            } catch (ErrnoException err) {
                if (err.errno != OsConstants.EINTR) throw err;
            }
        }
        return digest.digest();
    }

    private static MessageDigest sha() {
        try { return MessageDigest.getInstance("SHA-256"); }
        catch (NoSuchAlgorithmException impossible) { throw new AssertionError(impossible); }
    }

    private static void require(boolean requirement) throws IOException {
        if (!requirement) throw new IOException("The complete selected file could not be frozen");
    }
}
