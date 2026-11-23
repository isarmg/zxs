package org.zipworkspace.preview;

import android.content.Context;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.File;
import java.io.FileDescriptor;
import java.io.IOException;
import java.io.InputStream;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.regex.Pattern;

/** Private one-file Android draft records. No connection, credentials, or automatic submit. */
final class RemoteSaveJournal {
    private static final String STORE = "remote-saves";
    private static final int MAGIC = 0x52533331; // RS31
    private static final int MAX_BODY = PlainText.LIMIT;
    private static final int MAX_RECORD = MAX_BODY + 4096;
    private static final int RECORD_DIGEST_SIZE = 32;
    private static final Pattern ID = Pattern.compile("[0-9a-f]{32}");
    private static final Pattern SHA = Pattern.compile("[0-9a-f]{64}");
    private static final int READ = OsConstants.O_RDONLY | OsConstants.O_NOFOLLOW
            | OsConstants.O_CLOEXEC | OsConstants.O_NONBLOCK;
    private static final int CREATE = OsConstants.O_RDWR | OsConstants.O_CREAT | OsConstants.O_EXCL
            | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC;

    /** The account name is not a secret. Never put a password or authorization header here. */
    record Authority(String destinationIdentity, String account) {
        Authority {
            if (destinationIdentity == null || !SHA.matcher(destinationIdentity).matches()
                    || account == null || account.isEmpty()
                    || !StandardCharsets.UTF_8.newEncoder().canEncode(account)
                    || account.codePoints().anyMatch(Character::isISOControl)
                    || account.getBytes(StandardCharsets.UTF_8).length > 256) {
                throw new IllegalArgumentException("invalid original remote authority");
            }
        }
    }

    static final class Prepared {
        private final Authority authority;
        private final RemoteSaveContract.Save save;
        private final byte[] body;

        private Prepared(Authority authority, RemoteSaveContract.Save save, byte[] body) {
            this.authority = authority;
            this.save = save;
            this.body = body;
        }

        Authority authority() { return authority; }
        RemoteSaveContract.Save save() { return save; }

        /** A stream over the exact checked bytes; no later file or provider re-open. */
        InputStream body() { return new ByteArrayInputStream(body); }
    }

    private RemoteSaveJournal() {}

    /** Copy the already stopped, bounded local editor draft. The original editor is not modified. */
    static synchronized Prepared prepare(Context context, Authority authority,
                                         RemoteSaveContract.Save save, byte[] completeBytes) throws IOException {
        if (completeBytes == null || completeBytes.length > MAX_BODY || authority == null || save == null) {
            throw new IOException("invalid remote draft");
        }
        byte[] body = completeBytes.clone();
        requireBody(save, body);
        var prepared = new Prepared(authority, save, body);
        byte[] bytes = encode(prepared);
        return inStore(context, true, (home, local, target) -> {
            String operation = save.operation();
            File stage = new File(target.path, "." + operation + ".pending");
            File published = new File(target.path, fileName(operation));
            try (var original = new LocalWorkspaceStore.Handle(stage, CREATE, 0600)) {
                var created = Os.fstat(original.fd);
                checkFile(created, created, target, 1);
                need(created.st_size == 0);
                try (var out = new ParcelFileDescriptor.AutoCloseOutputStream(
                        ParcelFileDescriptor.dup(original.fd))) {
                    out.write(bytes);
                    out.flush();
                }
                var sealed = Os.fstat(original.fd);
                need(LocalWorkspaceStore.same(created, sealed));
                checkFile(Os.lstat(stage.getPath()), sealed, target, 1);
                byte[] readback = readExact(original.fd, sealed.st_size);
                need(Arrays.equals(bytes, readback));
                Prepared verified = decode(readback, operation, authority);
                Os.fsync(original.fd);
                home.check();
                local.check();
                target.check();
                checkFile(Os.lstat(stage.getPath()), sealed, target, 1);
                // Exclusive publication. Never remove or replace an occupied result.
                Os.link(stage.getPath(), published.getPath());
                target.check();
                checkFile(Os.lstat(published.getPath()), sealed, target, 2);
                checkFile(Os.lstat(stage.getPath()), sealed, target, 2);
                try (var names = target.openNames()) {
                    // This generated stage is the only removed name. On an ambiguous failure,
                    // keep both names; a later open validates the final name and single link.
                    checkFile(Os.lstat(stage.getPath()), sealed, target, 2);
                    names.deleteFile(stage.toPath().getFileName());
                }
                Os.fsync(original.fd);
                target.sync();
                home.check();
                local.check();
                target.check();
                var finalRead = Os.fstat(original.fd);
                checkFile(finalRead, sealed, target, 1);
                unchanged(Os.lstat(published.getPath()), finalRead, target);
                need(Arrays.equals(bytes, readExact(original.fd, finalRead.st_size)));
                unchanged(Os.fstat(original.fd), finalRead, target);
                unchanged(Os.lstat(published.getPath()), finalRead, target);
                return verified;
            }
        });
    }

    /** Reopen this exact original operation, not an arbitrary path or latest draft. */
    static synchronized Prepared reopen(Context context, Authority authority, String operation)
            throws IOException {
        String name = fileName(operation);
        return inStore(context, false, (home, local, target) -> {
            File published = new File(target.path, name);
            var named = Os.lstat(published.getPath());
            checkFile(named, named, target, 1);
            try (var held = new LocalWorkspaceStore.Handle(published, READ, 0)) {
                var before = Os.fstat(held.fd);
                unchanged(before, named, target);
                byte[] bytes = readExact(held.fd, before.st_size);
                unchanged(Os.fstat(held.fd), before, target);
                unchanged(Os.lstat(published.getPath()), before, target);
                Prepared verified = decode(bytes, operation, authority);
                // Reestablish directory durability after a possibly interrupted prepare.
                target.sync();
                local.sync();
                home.sync();
                unchanged(Os.lstat(published.getPath()), before, target);
                return verified;
            }
        });
    }

    private interface Access<T> {
        T run(LocalWorkspaceStore.Directory home, LocalWorkspaceStore.Directory local,
              LocalWorkspaceStore.Directory target) throws ErrnoException, IOException;
    }

    private static <T> T inStore(Context context, boolean create, Access<T> access) throws IOException {
        File homePath = context.getDataDir();
        int uid = Process.myUid();
        need(homePath != null && homePath.isAbsolute() && uid != 0);
        File localPath = new File(homePath, "no_backup");
        File storePath = new File(localPath, STORE);
        try (var home = new LocalWorkspaceStore.Directory(homePath, uid, false)) {
            if (create) need(localPath.equals(context.getNoBackupFilesDir()));
            try (var local = new LocalWorkspaceStore.Directory(localPath, uid, false)) {
                need(home.original.st_dev == local.original.st_dev);
                if (create) {
                    home.check();
                    local.check();
                    try {
                        Os.mkdir(storePath.getPath(), 0700);
                    } catch (ErrnoException exists) {
                        if (exists.errno != OsConstants.EEXIST) throw exists;
                    }
                }
                try (var target = new LocalWorkspaceStore.Directory(storePath, uid, true)) {
                    need(local.original.st_dev == target.original.st_dev);
                    home.check();
                    local.check();
                    target.check();
                    if (create) {
                        target.sync();
                        local.sync();
                        home.sync();
                    }
                    T verified = access.run(home, local, target);
                    home.check();
                    local.check();
                    target.check();
                    return verified;
                }
            }
        } catch (ErrnoException | SecurityException failure) {
            throw new IOException("unable to verify original remote draft", failure);
        }
    }

    private static String fileName(String operation) throws IOException {
        need(operation != null && ID.matcher(operation).matches() && !operation.equals("0".repeat(32)));
        return operation + ".save";
    }

    private static void checkFile(StructStat seen, StructStat original,
                                  LocalWorkspaceStore.Directory target, int links) throws IOException {
        need(OsConstants.S_ISREG(seen.st_mode) && (seen.st_mode & 07777) == 0600
                && seen.st_uid == target.uid && seen.st_dev == target.original.st_dev
                && LocalWorkspaceStore.same(seen, original) && seen.st_size == original.st_size
                && seen.st_nlink == links);
    }

    private static void unchanged(StructStat seen, StructStat original, LocalWorkspaceStore.Directory target)
            throws IOException {
        checkFile(seen, original, target, 1);
        need(seen.st_mtim.compareTo(original.st_mtim) == 0 && seen.st_ctim.compareTo(original.st_ctim) == 0);
    }

    private static byte[] readExact(FileDescriptor held, long length) throws IOException, ErrnoException {
        need(length > 0 && length <= MAX_RECORD);
        byte[] result = new byte[(int) length];
        for (int done = 0; done < result.length;) {
            int read = Os.pread(held, result, done, Math.min(65_536, result.length - done), done);
            need(read > 0 && read <= result.length - done);
            done += read;
        }
        need(Os.pread(held, new byte[1], 0, 1, length) == 0);
        return result;
    }

    private static byte[] encode(Prepared prepared) throws IOException {
        var save = prepared.save;
        var bytes = new ByteArrayOutputStream();
        try (var out = new DataOutputStream(bytes)) {
            out.writeInt(MAGIC);
            out.writeUTF(prepared.authority.destinationIdentity());
            out.writeUTF(prepared.authority.account());
            out.writeUTF(save.container());
            out.writeUTF(save.operation());
            out.writeUTF(save.node());
            out.writeUTF(save.baseSequence().toString());
            if (save.target() instanceof RemoteSaveContract.FirstMain first) {
                out.writeByte(1);
                out.writeUTF(first.name());
            } else if (save.target() instanceof RemoteSaveContract.BranchHead head) {
                out.writeByte(2);
                out.writeUTF(head.branch());
                out.writeUTF(head.expectedHead());
            } else if (save.target() instanceof RemoteSaveContract.HistoricalMain fork) {
                out.writeByte(3);
                out.writeUTF(fork.branch());
                out.writeUTF(fork.fromNode());
            } else {
                throw new IOException("unknown save target");
            }
            out.writeUTF(save.sha256());
            out.writeInt(prepared.body.length);
            out.write(prepared.body);
        }
        bytes.write(sha256(bytes.toByteArray()));
        byte[] record = bytes.toByteArray();
        need(record.length <= MAX_RECORD);
        return record;
    }

    private static Prepared decode(byte[] record, String expectedOperation, Authority expectedAuthority)
            throws IOException {
        need(record.length > RECORD_DIGEST_SIZE && record.length <= MAX_RECORD);
        int end = record.length - RECORD_DIGEST_SIZE;
        need(MessageDigest.isEqual(sha256(Arrays.copyOfRange(record, 0, end)),
                Arrays.copyOfRange(record, end, record.length)));
        try (var input = new DataInputStream(new ByteArrayInputStream(record, 0, end))) {
            need(input.readInt() == MAGIC);
            var authority = new Authority(input.readUTF(), input.readUTF());
            need(authority.equals(expectedAuthority));
            String container = input.readUTF();
            String operation = input.readUTF();
            need(operation.equals(expectedOperation));
            String node = input.readUTF();
            String sequence = input.readUTF();
            need(sequence.matches("0|[1-9][0-9]{0,19}"));
            RemoteSaveContract.Target target = switch (input.readUnsignedByte()) {
                case 1 -> new RemoteSaveContract.FirstMain(input.readUTF());
                case 2 -> new RemoteSaveContract.BranchHead(input.readUTF(), input.readUTF());
                case 3 -> new RemoteSaveContract.HistoricalMain(input.readUTF(), input.readUTF());
                default -> throw new IOException("unknown save target");
            };
            String digest = input.readUTF();
            int count = input.readInt();
            need(count >= 0 && count <= MAX_BODY && count == input.available());
            byte[] body = new byte[count];
            input.readFully(body);
            var save = new RemoteSaveContract.Save(container, operation, node, new BigInteger(sequence),
                    target, count, digest);
            requireBody(save, body);
            var original = new Prepared(authority, save, body);
            need(Arrays.equals(record, encode(original)));
            return original;
        } catch (IllegalArgumentException invalid) {
            throw new IOException("invalid original remote draft", invalid);
        }
    }

    private static void requireBody(RemoteSaveContract.Save save, byte[] bytes) throws IOException {
        String digest = HexFormat.of().formatHex(sha256(bytes));
        need(save.length() == bytes.length && save.sha256().equals(digest));
    }

    private static byte[] sha256(byte[] bytes) {
        try {
            return MessageDigest.getInstance("SHA-256").digest(bytes);
        } catch (NoSuchAlgorithmException impossible) {
            throw new AssertionError(impossible);
        }
    }

    private static void need(boolean condition) throws IOException {
        if (!condition) throw new IOException("unable to verify original remote draft");
    }
}
