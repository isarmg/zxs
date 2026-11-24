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
import java.nio.file.Path;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.List;
import java.util.TreeMap;
import java.util.regex.Pattern;

/** Private one-file Android draft records. No connection, credentials, or automatic submit. */
final class RemoteSaveJournal {
    private static final String STORE = "remote-saves";
    private static final int MAGIC = 0x52533331; // RS31
    private static final int STAGED_MAGIC = 0x52534631; // RSF1
    private static final int PLAN_MAGIC = 0x52535031; // RSP1
    private static final int ATTEMPT_MAGIC = 0x52534131; // RSA1
    private static final int RECEIPT_MAGIC = 0x52535231; // RSR1
    private static final int MAX_BODY = PlainText.LIMIT;
    private static final int MAX_RECORD = MAX_BODY + 4096;
    private static final int RECORD_DIGEST_SIZE = 32;
    private static final int PAGE_LIMIT = 24;
    private static final int NAME_SCAN_LIMIT = 4096;
    private static final Pattern PLAN_NAME = Pattern.compile("([0-9a-f]{32})\\.plan");
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
        private final OrdinaryFileStage.Staged staged;

        private Prepared(Authority authority, RemoteSaveContract.Save save, byte[] body) {
            this(authority, save, body, null);
        }

        private Prepared(Authority authority, RemoteSaveContract.Save save, byte[] body,
                         OrdinaryFileStage.Staged staged) {
            this.authority = authority;
            this.save = save;
            this.body = body;
            this.staged = staged;
        }

        Authority authority() { return authority; }
        RemoteSaveContract.Save save() { return save; }

        /** In-memory original, used by existing bounded-text fixtures. */
        InputStream body() { return new ByteArrayInputStream(body); }

        private InputStream openBody(Context context) throws IOException {
            return staged == null ? body() : OrdinaryFileStage.openBody(context, staged);
        }
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
        // The small independent selector is durable before body publication. A
        // crash can leave a visible original ID with an unreadable body, never a
        // picker entry that on its own grants send eligibility.
        writeRecord(context, nameFor(save.operation(), ".plan"),
                encodeMarker(new Plan(authority, save), PLAN_MAGIC, null));
        writeRecord(context, fileName(save.operation()), bytes);
        return prepared;
    }

    static synchronized void prepareStaged(Context context, Plan plan, OrdinaryFileStage.Staged stage)
            throws IOException {
        need(plan != null && stage != null && plan.authority() != null && plan.save() != null);
        var save = plan.save();
        need(stage.equals(new OrdinaryFileStage.Staged(save.operation(), save.length(), save.sha256())));
        try (InputStream verified = OrdinaryFileStage.openBody(context, stage)) {
            writeRecord(context, nameFor(save.operation(), ".plan"), encodeMarker(plan, STAGED_MAGIC, null));
        }
    }

    static synchronized void discardUnboundStage(Context context, OrdinaryFileStage.Staged stage)
            throws IOException {
        need(stage != null);
        String op = stage.id();
        // Include interrupted journal names: ambiguity means retaining the body.
        inStore(context, true, (home, local, target) -> {
            for (String suffix : List.of(".plan", ".save", ".attempt", ".receipt")) {
                String finalName = nameFor(op, suffix);
                for (String leaf : List.of(finalName, stageName(finalName))) {
                    try {
                        Os.lstat(new File(target.path, leaf).getPath());
                        throw new IOException("Original operation may already refer to this body");
                    } catch (ErrnoException absent) {
                        if (absent.errno != OsConstants.ENOENT) throw absent;
                    }
                }
            }
            home.check();
            local.check();
            target.check();
            OrdinaryFileStage.discardUnbound(context, stage);
            return null;
        });
    }

    record Plan(Authority authority, RemoteSaveContract.Save save) {}
    record Page(List<Plan> rows, String next) {}

    /**
     * Only this verified app-private directory, exact authority and one bounded
     * page. No user/provider path, payload read, credentials, or network access.
     */
    static synchronized Page plans(Context context, Authority authority, String after) throws IOException {
        need(authority != null);
        if (after != null) nameFor(after, ".plan"); // Validate the cursor before filesystem work.
        return inStore(context, false, (home, local, target) -> {
            var page = new TreeMap<String, Plan>();
            int visited = 0;
            try (var names = target.openNames()) {
                for (Path entry : names) {
                    need(++visited <= NAME_SCAN_LIMIT && !Thread.currentThread().isInterrupted());
                    String leaf = entry.getFileName().toString();
                    var matcher = PLAN_NAME.matcher(leaf);
                    if (!matcher.matches()) continue;
                    String operation = matcher.group(1);
                    if (after != null && operation.compareTo(after) <= 0) continue;
                    if (page.size() > PAGE_LIMIT && operation.compareTo(page.lastKey()) >= 0) continue;
                    target.check();
                    var selected = readSelector(target, leaf, operation, null, false);
                    target.check();
                    if (selected == null) continue;
                    Plan checked = selected.plan();
                    if (!authority.equals(checked.authority())) continue;
                    page.put(operation, checked);
                    if (page.size() > PAGE_LIMIT + 1) page.pollLastEntry();
                }
            }
            target.sync();
            String next = null;
            if (page.size() > PAGE_LIMIT) {
                page.pollLastEntry();
                next = page.lastKey();
            }
            return new Page(List.copyOf(page.values()), next);
        });
    }

    /** An attempted submission can be looked up without reading its frozen body. */
    static synchronized Plan attempted(Context context, Authority authority, String operation) throws IOException {
        byte[] record = readRecord(context, nameFor(operation, ".attempt"), true);
        return record == null ? null : decodeMarker(record, authority, operation, ATTEMPT_MAGIC).plan();
    }

    static synchronized Prepared beginAttempt(Context context, Prepared proposed) throws IOException {
        need(proposed.staged == null);
        var save = proposed.save();
        var prepared = reopen(context, proposed.authority(), save.operation());
        need(save.equals(prepared.save()) && Arrays.equals(proposed.body, prepared.body)
                && java.util.Objects.equals(proposed.staged, prepared.staged));
        markAttempt(context, prepared);
        return prepared;
    }

    static synchronized InputStream beginWithBody(Context context, Plan original) throws IOException {
        var prepared = reopen(context, original.authority(), original.save().operation());
        need(original.equals(new Plan(prepared.authority(), prepared.save())));
        var held = prepared.openBody(context);
        try {
            markAttempt(context, prepared);
            return held;
        } catch (IOException | RuntimeException failure) {
            try { held.close(); } catch (IOException close) { failure.addSuppressed(close); }
            throw failure;
        }
    }

    private static void markAttempt(Context context, Prepared prepared) throws IOException {
        var save = prepared.save();
        Plan plan = new Plan(prepared.authority(), prepared.save());
        Plan existing = attempted(context, plan.authority(), save.operation());
        if (existing == null) {
            writeRecord(context, nameFor(save.operation(), ".attempt"),
                    encodeMarker(plan, ATTEMPT_MAGIC, null));
        } else {
            need(plan.equals(existing));
        }
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
        String leaf = nameFor(operation, ".plan");
        var selector = inStore(context, false, (home, local, target) -> {
            var recovered = readSelector(target, leaf, operation, authority, true);
            if (recovered != null) {
                local.sync();
                home.sync();
            }
            return recovered;
        });
        if (selector != null && selector.staged()) {
            need(readRecord(context, fileName(operation), true) == null);
            return stagedPrepared(selector.plan());
        }
        byte[] bytes = readRecord(context, fileName(operation), false);
        if (recordKind(bytes) == STAGED_MAGIC) {
            // Read existing two-record ordinary operations without rewriting them.
            Plan checked = decodeMarker(bytes, authority, operation, STAGED_MAGIC).plan();
            need(selector != null && checked.equals(selector.plan()));
            return stagedPrepared(checked);
        }
        return decode(bytes, operation, authority);
    }

    private static Prepared stagedPrepared(Plan plan) {
        var save = plan.save();
        return new Prepared(plan.authority(), save, null,
                new OrdinaryFileStage.Staged(save.operation(), save.length(), save.sha256()));
    }

    private record Selector(Plan plan, boolean staged) {}

    private static int recordKind(byte[] record) throws IOException {
        need(record.length >= Integer.BYTES);
        return java.nio.ByteBuffer.wrap(record).getInt();
    }

    private static Selector readSelector(LocalWorkspaceStore.Directory target, String leaf,
                                         String operation, Authority authority, boolean repair)
            throws ErrnoException, IOException {
        File published = new File(target.path, leaf);
        File pending = new File(target.path, stageName(leaf));
        StructStat named;
        try {
            named = Os.lstat(published.getPath());
        } catch (ErrnoException absent) {
            if (absent.errno != OsConstants.ENOENT) throw absent;
            // A lone pending name was never published and cannot qualify.
            return null;
        }
        long links = named.st_nlink;
        need(links == 1 || links == 2);
        if (links == 1) {
            byte[] bytes = readNamed(target, published, named, 4096);
            int kind = recordKind(bytes);
            need(kind == PLAN_MAGIC || kind == STAGED_MAGIC);
            if (repair) target.sync();
            return new Selector(decodeMarker(bytes, authority, operation, kind).plan(), kind == STAGED_MAGIC);
        }

        // The one recoverable publish window is two names for the already
        // checked staged marker inode. Text and attempted/receipt records are unchanged.
        checkFile(named, named, target, 2);
        need(named.st_size > 0 && named.st_size <= 4096);
        try (var held = new LocalWorkspaceStore.Handle(published, READ, 0)) {
            unchanged(Os.fstat(held.fd), named, target, 2);
            unchanged(Os.lstat(pending.getPath()), named, target, 2);
            byte[] bytes = readExact(held.fd, named.st_size);
            Plan exact = decodeMarker(bytes, authority, operation, STAGED_MAGIC).plan();
            target.check();
            unchanged(Os.fstat(held.fd), named, target, 2);
            unchanged(Os.lstat(published.getPath()), named, target, 2);
            unchanged(Os.lstat(pending.getPath()), named, target, 2);
            if (repair) {
                try (var names = target.openNames()) {
                    unchanged(Os.lstat(pending.getPath()), named, target, 2);
                    names.deleteFile(pending.toPath().getFileName());
                }
                Os.fsync(held.fd);
                target.sync();
                var after = Os.fstat(held.fd);
                checkFile(after, named, target, 1);
                unchanged(Os.lstat(published.getPath()), after, target);
            }
            return new Selector(exact, true);
        }
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
            byte[] bytes = readNamed(target, published, named, MAX_RECORD);
            // Reestablish namespace durability after an interrupted operation.
            target.sync();
            local.sync();
            home.sync();
            unchanged(Os.lstat(published.getPath()), named, target);
            return bytes;
        });
    }

    private static byte[] readNamed(LocalWorkspaceStore.Directory target, File original, int max)
            throws IOException, ErrnoException {
        return readNamed(target, original, Os.lstat(original.getPath()), max);
    }

    private static byte[] readNamed(LocalWorkspaceStore.Directory target, File original, StructStat named, int max)
            throws IOException, ErrnoException {
        checkFile(named, named, target, 1);
        need(named.st_size > 0 && named.st_size <= max);
        try (var held = new LocalWorkspaceStore.Handle(original, READ, 0)) {
            var before = Os.fstat(held.fd);
            unchanged(before, named, target);
            byte[] bytes = readExact(held.fd, before.st_size);
            unchanged(Os.fstat(held.fd), before, target);
            unchanged(Os.lstat(original.getPath()), before, target);
            return bytes;
        }
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
        unchanged(seen, original, target, 1);
    }

    private static void unchanged(StructStat seen, StructStat original, LocalWorkspaceStore.Directory target,
                                  int links) throws IOException {
        checkFile(seen, original, target, links);
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
            need(plan.save().length() <= MAX_BODY);
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
        if (save.length() <= MAX_BODY) {
            out.writeInt((int) save.length());
        } else {
            out.writeInt(-1);
            out.writeLong(save.length());
        }
    }

    private static Plan readPlan(DataInputStream input, String expectedOperation, Authority expectedAuthority)
            throws IOException {
        var authority = new Authority(input.readUTF(), input.readUTF());
        need(expectedAuthority == null || authority.equals(expectedAuthority));
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
        int mark = input.readInt();
        long count = mark == -1 ? input.readLong() : mark;
        need(mark == -1 ? count > MAX_BODY : count >= 0 && count <= MAX_BODY);
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
