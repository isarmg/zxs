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
    private static final int ATTEMPT_MAGIC = 0x52534131; // RSA1
    private static final int RECEIPT_MAGIC = 0x52535231; // RSR1
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
        writeRecord(context, fileName(save.operation()), bytes);
        return prepared;
    }

    record Plan(Authority authority, RemoteSaveContract.Save save) {}

    /** An attempted submission can be looked up without reading its frozen body. */
    static synchronized Plan attempted(Context context, Authority authority, String operation) throws IOException {
        byte[] record = readRecord(context, nameFor(operation, ".attempt"), true);
        return record == null ? null : decodeMarker(record, authority, operation, ATTEMPT_MAGIC).plan();
    }

    static synchronized Prepared beginAttempt(Context context, Prepared proposed) throws IOException {
        var save = proposed.save();
        var prepared = reopen(context, proposed.authority(), save.operation());
        need(save.equals(prepared.save()) && Arrays.equals(proposed.body, prepared.body));
        Plan plan = new Plan(prepared.authority(), prepared.save());
        Plan existing = attempted(context, plan.authority(), save.operation());
        if (existing == null) {
            writeRecord(context, nameFor(save.operation(), ".attempt"),
                    encodeMarker(plan, ATTEMPT_MAGIC, null));
        } else {
            need(plan.equals(existing));
        }
        return prepared;
    }

    static synchronized BigInteger receipt(Context context, Plan original) throws IOException {
        byte[] record = readRecord(context, nameFor(original.save().operation(), ".receipt"), true);
        if (record == null) return null;
        need(original.equals(attempted(context, original.authority(), original.save().operation())));
        Marker intake = decodeMarker(record, original.authority(), original.save().operation(), RECEIPT_MAGIC);
        need(original.equals(intake.plan()));
        return intake.sequence();
    }

    static synchronized void settle(Context context, Plan original, BigInteger sequence) throws IOException {
        validateSequence(original, sequence);
        need(original.equals(attempted(context, original.authority(), original.save().operation())));
        var previous = receipt(context, original);
        if (previous != null) {
            need(previous.equals(sequence));
            return;
        }
        writeRecord(context, nameFor(original.save().operation(), ".receipt"),
                encodeMarker(original, RECEIPT_MAGIC, sequence));
    }

    private static void writeRecord(Context context, String finalName, byte[] bytes) throws IOException {
        inStore(context, true, (home, local, target) -> {
            File stage = new File(target.path, stageName(finalName));
            File published = new File(target.path, finalName);
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
                unchanged(Os.lstat(stage.getPath()), sealed, target);
                need(Arrays.equals(bytes, readExact(original.fd, sealed.st_size)));
                unchanged(Os.fstat(original.fd), sealed, target);
                Os.fsync(original.fd);
                home.check();
                local.check();
                target.check();
                unchanged(Os.lstat(stage.getPath()), sealed, target);
                // Both names are derived from one exact app-owned record. Do not
                // retry or clean a stage with an uncertain publication outcome.
                Os.link(stage.getPath(), published.getPath());
                target.check();
                checkFile(Os.lstat(published.getPath()), sealed, target, 2);
                checkFile(Os.lstat(stage.getPath()), sealed, target, 2);
                try (var names = target.openNames()) {
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
                return null;
            }
        });
    }

    /** Reopen this exact original operation, not an arbitrary path or latest draft. */
    static synchronized Prepared reopen(Context context, Authority authority, String operation)
            throws IOException {
        byte[] bytes = readRecord(context, fileName(operation), false);
        return decode(bytes, operation, authority);
    }

    private static byte[] readRecord(Context context, String name, boolean optional) throws IOException {
        return inStore(context, false, (home, local, target) -> {
            File published = new File(target.path, name);
            StructStat named;
            try {
                named = Os.lstat(published.getPath());
            } catch (ErrnoException absent) {
                if (!optional || absent.errno != OsConstants.ENOENT) throw absent;
                File stage = new File(target.path, stageName(name));
                try {
                    Os.lstat(stage.getPath());
                } catch (ErrnoException stageAbsent) {
                    if (stageAbsent.errno == OsConstants.ENOENT) return null;
                    throw stageAbsent;
                }
                throw new IOException("original record publication remains unresolved");
            }
            checkFile(named, named, target, 1);
            try (var held = new LocalWorkspaceStore.Handle(published, READ, 0)) {
                var before = Os.fstat(held.fd);
                unchanged(before, named, target);
                byte[] bytes = readExact(held.fd, before.st_size);
                unchanged(Os.fstat(held.fd), before, target);
                unchanged(Os.lstat(published.getPath()), before, target);
                // Reestablish namespace durability after an interrupted operation.
                target.sync();
                local.sync();
                home.sync();
                unchanged(Os.lstat(published.getPath()), before, target);
                return bytes;
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
        return nameFor(operation, ".save");
    }

    private static String nameFor(String operation, String suffix) throws IOException {
        need(operation != null && ID.matcher(operation).matches() && !operation.equals("0".repeat(32)));
        return operation + suffix;
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

    private static String stageName(String name) {
        // Keep the established original-body staging name across the ledger change.
        String stem = name.endsWith(".save") ? name.substring(0, name.length() - 5) : name;
        return "." + stem + ".pending";
    }

    private static byte[] encode(Prepared prepared) throws IOException {
        var bytes = new ByteArrayOutputStream();
        try (var out = new DataOutputStream(bytes)) {
            out.writeInt(MAGIC);
            writePlan(out, new Plan(prepared.authority, prepared.save));
            out.write(prepared.body);
        }
        return sealedRecord(bytes);
    }

    private static Prepared decode(byte[] record, String expectedOperation, Authority expectedAuthority)
            throws IOException {
        try (var input = openRecord(record, MAGIC)) {
            Plan plan = readPlan(input, expectedOperation, expectedAuthority);
            int count = (int) plan.save().length();
            need(count == input.available());
            byte[] body = new byte[count];
            input.readFully(body);
            requireBody(plan.save(), body);
            var original = new Prepared(plan.authority(), plan.save(), body);
            need(Arrays.equals(record, encode(original)));
            return original;
        } catch (IllegalArgumentException invalid) {
            throw new IOException("invalid original remote draft", invalid);
        }
    }

    private record Marker(Plan plan, BigInteger sequence) {}

    private static byte[] encodeMarker(Plan plan, int kind, BigInteger sequence) throws IOException {
        var bytes = new ByteArrayOutputStream();
        try (var out = new DataOutputStream(bytes)) {
            out.writeInt(kind);
            writePlan(out, plan);
            if (kind == RECEIPT_MAGIC) out.writeUTF(sequence.toString());
        }
        return sealedRecord(bytes);
    }

    private static Marker decodeMarker(byte[] record, Authority authority, String operation, int kind)
            throws IOException {
        try (var input = openRecord(record, kind)) {
            Plan plan = readPlan(input, operation, authority);
            BigInteger sequence = null;
            if (kind == RECEIPT_MAGIC) {
                sequence = readSequence(input);
                validateSequence(plan, sequence);
            }
            need(input.read() == -1 && Arrays.equals(record, encodeMarker(plan, kind, sequence)));
            return new Marker(plan, sequence);
        } catch (IllegalArgumentException invalid) {
            throw new IOException("invalid original remote record", invalid);
        }
    }

    private static void validateSequence(Plan plan, BigInteger sequence) throws IOException {
        need(sequence != null && sequence.compareTo(plan.save().baseSequence()) > 0
                && sequence.compareTo(BigInteger.ONE.shiftLeft(64).subtract(BigInteger.ONE)) < 0);
    }

    private static byte[] sealedRecord(ByteArrayOutputStream bytes) throws IOException {
        bytes.write(sha256(bytes.toByteArray()));
        byte[] record = bytes.toByteArray();
        need(record.length <= MAX_RECORD);
        return record;
    }

    private static DataInputStream openRecord(byte[] record, int kind) throws IOException {
        need(record.length > RECORD_DIGEST_SIZE + 4 && record.length <= MAX_RECORD);
        int end = record.length - RECORD_DIGEST_SIZE;
        need(MessageDigest.isEqual(sha256(Arrays.copyOfRange(record, 0, end)),
                Arrays.copyOfRange(record, end, record.length)));
        var input = new DataInputStream(new ByteArrayInputStream(record, 0, end));
        need(input.readInt() == kind);
        return input;
    }

    private static void writePlan(DataOutputStream out, Plan plan) throws IOException {
        RemoteSaveContract.Save save = plan.save();
        need(save.length() <= MAX_BODY);
        out.writeUTF(plan.authority().destinationIdentity());
        out.writeUTF(plan.authority().account());
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
        out.writeInt((int) save.length());
    }

    private static Plan readPlan(DataInputStream input, String expectedOperation, Authority expectedAuthority)
            throws IOException {
        var authority = new Authority(input.readUTF(), input.readUTF());
        need(authority.equals(expectedAuthority));
        String container = input.readUTF();
        String operation = input.readUTF();
        need(operation.equals(expectedOperation));
        String node = input.readUTF();
        BigInteger sequence = readSequence(input);
        RemoteSaveContract.Target target = switch (input.readUnsignedByte()) {
            case 1 -> new RemoteSaveContract.FirstMain(input.readUTF());
            case 2 -> new RemoteSaveContract.BranchHead(input.readUTF(), input.readUTF());
            case 3 -> new RemoteSaveContract.HistoricalMain(input.readUTF(), input.readUTF());
            default -> throw new IOException("unknown save target");
        };
        String digest = input.readUTF();
        int count = input.readInt();
        need(count >= 0 && count <= MAX_BODY);
        return new Plan(authority,
                new RemoteSaveContract.Save(container, operation, node, sequence, target, count, digest));
    }

    private static BigInteger readSequence(DataInputStream input) throws IOException {
        String sequence = input.readUTF();
        need(sequence.matches("0|[1-9][0-9]{0,19}"));
        return new BigInteger(sequence);
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
