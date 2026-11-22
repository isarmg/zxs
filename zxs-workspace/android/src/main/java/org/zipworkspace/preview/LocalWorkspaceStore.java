package org.zipworkspace.preview;

import android.content.Context;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import java.io.File;
import java.io.FileDescriptor;
import java.io.IOException;
import java.io.OutputStream;

/** The only writer for a new local Android snapshot. No caller-supplied paths or imported documents. */
final class LocalWorkspaceStore {
    private static final String STORE = "workspaces";
    private static final long MAX_ARCHIVE = 2L << 20;
    private static final int DIRECTORY = OsConstants.O_RDONLY | OsConstants.O_DIRECTORY
            | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC;
    private static final int CANDIDATE = OsConstants.O_RDWR | OsConstants.O_CREAT | OsConstants.O_EXCL
            | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC;

    enum Status { COMMITTED, CONFLICT, NOT_COMMITTED, UNCONFIRMED }
    record Receipt(Status status, String filename) {}

    private LocalWorkspaceStore() {}

    static synchronized Receipt commit(Context context, LocalWorkspaceDraft.Frozen frozen) {
        Attempt enclosed = new Attempt(frozen);
        try {
            enclosed.write(context);
            return new Receipt(Status.COMMITTED, frozen.filename());
        } catch (ErrnoException | IOException | SecurityException err) {
            return new Receipt(enclosed.failure, frozen.filename());
        }
    }

    private static final class Handle implements AutoCloseable {
        final FileDescriptor fd;
        Handle(File path, int flags, int mode) throws ErrnoException {
            fd = Os.open(path.getPath(), flags, mode);
        }
        @Override public void close() throws ErrnoException { Os.close(fd); }
    }

    private static boolean same(StructStat a, StructStat b) {
        return a.st_ino != 0 && a.st_dev == b.st_dev && a.st_ino == b.st_ino && a.st_uid == b.st_uid;
    }

    private static void need(boolean ok) throws IOException {
        if (!ok) throw new IOException("The original local storage could not be verified");
    }

    private static final class Directory implements AutoCloseable {
        final File path;
        final Handle handle;
        final StructStat original;
        final int uid;
        final boolean privateChild;

        Directory(File path, int uid, boolean privateChild) throws ErrnoException, IOException {
            this.path = path;
            this.uid = uid;
            this.privateChild = privateChild;
            handle = new Handle(path, DIRECTORY, 0);
            try {
                original = Os.fstat(handle.fd);
                check();
            } catch (ErrnoException | IOException err) {
                try { handle.close(); }
                catch (ErrnoException close) { err.addSuppressed(close); }
                throw err;
            }
        }

        void check() throws ErrnoException, IOException {
            // These directories are under Android's application sandbox. 0771
            // on a framework directory is fine only when its writable group is
            // the app's own UID. Our app-owned child remains exactly 0700.
            StructStat named = Os.lstat(path.getPath());
            int mode = named.st_mode & 07777;
            need(OsConstants.S_ISDIR(named.st_mode) && named.st_uid == uid && same(original, named)
                    && (privateChild ? mode == 0700
                        : (mode & 0002) == 0 && ((mode & 0020) == 0 || named.st_gid == uid)));
        }

        void sync() throws ErrnoException { Os.fsync(handle.fd); }

        @Override public void close() throws ErrnoException { handle.close(); }
    }

    private static final class Attempt {
        final LocalWorkspaceDraft.Frozen frozen;
        Status failure = Status.NOT_COMMITTED;

        Attempt(LocalWorkspaceDraft.Frozen frozen) { this.frozen = frozen; }

        void write(Context context) throws ErrnoException, IOException {
            File data = context.getDataDir();
            int uid = Process.myUid();
            need(data != null && data.isAbsolute() && uid != 0);
            try (Directory home = new Directory(data, uid, false)) {
                File noBackup = context.getNoBackupFilesDir();
                need(new File(data, "no_backup").equals(noBackup));
                try (Directory local = new Directory(noBackup, uid, false)) {
                    need(home.original.st_dev == local.original.st_dev);
                    File store = new File(noBackup, STORE);
                    try { Os.mkdir(store.getPath(), 0700); }
                    catch (ErrnoException err) { if (err.errno != OsConstants.EEXIST) throw err; }
                    try (Directory target = new Directory(store, uid, true)) {
                        need(local.original.st_dev == target.original.st_dev);
                        home.check(); local.check(); target.check();
                        // Includes the namespace entries for a newly provisioned no-backup/store directory.
                        target.sync(); local.sync(); home.sync();
                        writeOne(home, local, target);
                    }
                }
            }
        }

        private void writeOne(Directory home, Directory local, Directory target) throws IOException, ErrnoException {
            File pending = new File(target.path, "." + frozen.operation + ".pending");
            File destination = new File(target.path, frozen.filename());
            try (Handle original = new Handle(pending, CANDIDATE, 0600)) {
                StructStat made = Os.fstat(original.fd);
                try {
                    checkFile(made, made, target, 1);
                    need(made.st_size == 0);
                    try (OutputStream output = new ParcelFileDescriptor.AutoCloseOutputStream(
                            ParcelFileDescriptor.dup(original.fd))) {
                        frozen.writeTo(output);
                        output.flush();
                    }
                    StructStat sealed = Os.fstat(original.fd);
                    need(same(made, sealed) && sealed.st_size >= 22 && sealed.st_size <= MAX_ARCHIVE);
                    checkFile(Os.lstat(pending.getPath()), sealed, target, 1);
                    verify(original.fd, sealed.st_size);
                    Os.fsync(original.fd);
                    home.check(); local.check(); target.check();
                    checkFile(Os.lstat(pending.getPath()), sealed, target, 1);

                    // Both generated names are in the one app-only directory.
                    // link(2) exclusively claims a new name for the original inode.
                    failure = Status.UNCONFIRMED;
                    try { Os.link(pending.getPath(), destination.getPath()); }
                    catch (ErrnoException err) {
                        if (err.errno == OsConstants.EEXIST) failure = Status.CONFLICT;
                        throw err;
                    }
                    target.check();
                    checkFile(Os.lstat(destination.getPath()), sealed, target, 2);
                    checkFile(Os.lstat(pending.getPath()), sealed, target, 2);
                    Os.unlink(pending.getPath());
                    Os.fsync(original.fd);
                    target.sync();
                    home.check(); local.check(); target.check();
                    checkFile(Os.lstat(destination.getPath()), sealed, target, 1);
                    checkFile(Os.fstat(original.fd), sealed, target, 1);
                } finally {
                    if (failure == Status.NOT_COMMITTED || failure == Status.CONFLICT) {
                        // A definite refusal can discard only this operation's still-original scratch name.
                        try {
                            target.check();
                            StructStat scratch = Os.lstat(pending.getPath());
                            if (OsConstants.S_ISREG(scratch.st_mode) && same(scratch, made)
                                    && scratch.st_nlink == 1) Os.unlink(pending.getPath());
                        } catch (ErrnoException | IOException ignored) {
                            // Nothing else is removed, and an uncertain publication is never cleaned here.
                        }
                    }
                }
            }
        }

        private void verify(FileDescriptor original, long length) throws IOException {
            OriginalBytes source = new OriginalBytes(original, length);
            ArchiveIndex.SnapshotMarker seen = ArchiveIndex.read(source, length).verifySnapshot(source);
            need(seen.generation == 1 && seen.parent == null && seen.repo.equals(frozen.repo)
                    && seen.branch.equals(frozen.branch) && seen.operation.equals(frozen.operation)
                    && seen.tree.equals(frozen.tree));
        }
    }

    private static void checkFile(StructStat seen, StructStat original, Directory dir, int links) throws IOException {
        need(OsConstants.S_ISREG(seen.st_mode) && (seen.st_mode & 07777) == 0600 && seen.st_uid == dir.uid
                && seen.st_dev == dir.original.st_dev && same(seen, original)
                && seen.st_size == original.st_size && seen.st_nlink == links);
    }

    private record OriginalBytes(FileDescriptor fd, long length) implements ArchiveIndex.MemberSource {
        @Override public void check() {}
        @Override public byte[] read(long offset, int count) throws IOException {
            byte[] bytes = new byte[count];
            fill(offset, bytes, count);
            return bytes;
        }
        @Override public void fill(long offset, byte[] bytes, int count) throws IOException {
            need(offset >= 0 && count >= 0 && count <= bytes.length && offset <= length - count);
            try {
                for (int done = 0; done < count;) {
                    int read = Os.pread(fd, bytes, done, Math.min(65_536, count - done), offset + done);
                    need(read > 0 && read <= count - done);
                    done += read;
                }
            } catch (ErrnoException err) { throw err.rethrowAsIOException(); }
        }
    }
}
