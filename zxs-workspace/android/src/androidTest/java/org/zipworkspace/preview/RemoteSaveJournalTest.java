package org.zipworkspace.preview;

import android.content.Context;
import android.system.Os;
import androidx.test.platform.app.InstrumentationRegistry;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.UUID;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertThrows;

public final class RemoteSaveJournalTest {
    private static final RemoteSaveJournal.Authority DESTINATION =
            new RemoteSaveJournal.Authority("55".repeat(32), "alice");

    @Test
    public void exactOriginalReopensIndependentlyOfCallerBuffer() throws Exception {
        byte[] bytes = "整份正文\r\n".getBytes(StandardCharsets.UTF_8);
        byte[] expected = bytes.clone();
        var save = save(bytes);
        Context context = context();
        var durable = RemoteSaveJournal.prepare(context, DESTINATION, save, bytes);
        bytes[0] ^= 1;
        assertEquals(save, durable.save());
        assertArrayEquals(expected, durable.body().readAllBytes());
        var reopened = RemoteSaveJournal.reopen(context, DESTINATION, save.operation());
        assertEquals(save, reopened.save());
        assertEquals(DESTINATION, reopened.authority());
        assertArrayEquals(expected, reopened.body().readAllBytes());
        assertThrows(IOException.class, () -> RemoteSaveJournal.reopen(context,
                new RemoteSaveJournal.Authority(DESTINATION.destinationIdentity(), "Alice"), save.operation()));
        assertThrows(IOException.class, () -> RemoteSaveJournal.reopen(context,
                new RemoteSaveJournal.Authority("44".repeat(32), "alice"), save.operation()));
    }

    @Test
    public void collisionCannotReplaceOriginalBodyOrOperation() throws Exception {
        var original = save(new byte[] {7});
        var replacement = new RemoteSaveContract.Save(original.container(), original.operation(),
                original.node(), original.baseSequence(), original.target(), 1, digest(new byte[] {8}));
        Context context = context();
        RemoteSaveJournal.prepare(context, DESTINATION, original, new byte[] {7});
        assertThrows(IOException.class,
                () -> RemoteSaveJournal.prepare(context, DESTINATION, replacement, new byte[] {8}));
        var reopened = RemoteSaveJournal.reopen(context, DESTINATION, original.operation());
        assertEquals(original, reopened.save());
        assertArrayEquals(new byte[] {7}, reopened.body().readAllBytes());
    }

    @Test
    public void unqualifiedOrDamagedOriginalCannotBecomePrepared() throws Exception {
        var original = save(new byte[] {7});
        Context context = context();
        assertThrows(IOException.class,
                () -> RemoteSaveJournal.prepare(context, DESTINATION, original, new byte[] {6}));
        assertThrows(IOException.class,
                () -> RemoteSaveJournal.reopen(context, DESTINATION, original.operation()));
        RemoteSaveJournal.prepare(context, DESTINATION, original, new byte[] {7});
        var file = new File(new File(context.getNoBackupFilesDir(), "remote-saves"),
                original.operation() + ".save");
        // Simulate a damaged metadata byte in this test-created record, leaving the whole body intact.
        try (var output = new RandomAccessFile(file, "rw")) {
            output.seek(6);
            output.write('4');
        }
        assertThrows(IOException.class,
                () -> RemoteSaveJournal.reopen(context, DESTINATION, original.operation()));
        var badMode = save(new byte[] {9});
        RemoteSaveJournal.prepare(context, DESTINATION, badMode, new byte[] {9});
        var modeFile = new File(file.getParentFile(), badMode.operation() + ".save");
        Os.chmod(modeFile.getPath(), 0644);
        assertThrows(IOException.class,
                () -> RemoteSaveJournal.reopen(context, DESTINATION, badMode.operation()));
    }

    @Test
    public void emptyBodyIsARealSaveAndNotNull() throws Exception {
        var original = save(new byte[0]);
        Context context = context();
        assertThrows(IOException.class, () -> RemoteSaveJournal.prepare(context, DESTINATION, original, null));
        RemoteSaveJournal.prepare(context, DESTINATION, original, new byte[0]);
        var reopened = RemoteSaveJournal.reopen(context, DESTINATION, original.operation());
        assertArrayEquals(new byte[0], reopened.body().readAllBytes());
    }

    @Test
    public void originalAttemptAndReceiptRemainQueryableWithoutUsableBody() throws Exception {
        byte[] first = {9, 7};
        var save = save(first);
        Context context = context();
        var prepared = RemoteSaveJournal.prepare(context, DESTINATION, save, first);
        assertNull(RemoteSaveJournal.attempted(context, DESTINATION, save.operation()));
        RemoteSaveJournal.beginAttempt(context, prepared);
        var original = RemoteSaveJournal.attempted(context, DESTINATION, save.operation());
        assertEquals(new RemoteSaveJournal.Plan(DESTINATION, save), original);

        var file = new File(store(context), save.operation() + ".save");
        try (var body = new RandomAccessFile(file, "rw")) {
            body.setLength(0);
        }
        assertThrows(IOException.class,
                () -> RemoteSaveJournal.reopen(context, DESTINATION, save.operation()));
        assertEquals(original, RemoteSaveJournal.attempted(context, DESTINATION, save.operation()));
        assertNull(RemoteSaveJournal.receipt(context, original));
        RemoteSaveJournal.settle(context, original, BigInteger.ONE);
        assertEquals(BigInteger.ONE, RemoteSaveJournal.receipt(context, original));
        RemoteSaveJournal.settle(context, original, BigInteger.ONE);
        assertThrows(IOException.class, () -> RemoteSaveJournal.settle(context, original, BigInteger.TWO));
        assertEquals(BigInteger.ONE, RemoteSaveJournal.receipt(context, original));
        assertThrows(IOException.class, () -> RemoteSaveJournal.attempted(context,
                new RemoteSaveJournal.Authority(DESTINATION.destinationIdentity(), "bob"), save.operation()));
    }

    @Test
    public void interruptedAttemptPublicationIsNotAbsent() throws Exception {
        var save = save(new byte[] {7});
        Context context = context();
        var prepared = RemoteSaveJournal.prepare(context, DESTINATION, save, new byte[] {7});
        var stage = new File(store(context), "." + save.operation() + ".attempt.pending");
        try (var file = new RandomAccessFile(stage, "rw")) {
            file.write(1);
        }
        assertThrows(IOException.class,
                () -> RemoteSaveJournal.attempted(context, DESTINATION, save.operation()));
        assertThrows(IOException.class, () -> RemoteSaveJournal.beginAttempt(context, prepared));
        assertTrue(stage.isFile());
    }

    @Test
    public void repeatedExactAttemptReusesMarkerAndRequiresOriginalBody() throws Exception {
        var save = save(new byte[] {7});
        Context context = context();
        var prepared = RemoteSaveJournal.prepare(context, DESTINATION, save, new byte[] {7});
        RemoteSaveJournal.beginAttempt(context, prepared);
        var again = RemoteSaveJournal.beginAttempt(context, prepared);
        assertEquals(save, again.save());
        assertArrayEquals(new byte[] {7}, again.body().readAllBytes());
        assertFalse(new File(store(context), "." + save.operation() + ".attempt.pending").exists());
        try (var body = new RandomAccessFile(new File(store(context), save.operation() + ".save"), "rw")) {
            body.setLength(0);
        }
        assertThrows(IOException.class, () -> RemoteSaveJournal.beginAttempt(context, prepared));
        assertEquals(save, RemoteSaveJournal.attempted(context, DESTINATION, save.operation()).save());
    }

    @Test
    public void noReceiptWithoutExactIndependentAttempt() throws Exception {
        var save = save(new byte[] {1});
        Context context = context();
        var prepared = RemoteSaveJournal.prepare(context, DESTINATION, save, new byte[] {1});
        var plan = new RemoteSaveJournal.Plan(DESTINATION, save);
        assertThrows(IOException.class, () -> RemoteSaveJournal.settle(context, plan, BigInteger.ONE));
        assertNull(RemoteSaveJournal.receipt(context, plan));
        RemoteSaveJournal.beginAttempt(context, prepared);
        var other = new RemoteSaveContract.Save(save.container(), save.operation(), save.node(), save.baseSequence(),
                new RemoteSaveContract.FirstMain("same-op-wrong-original.txt"), save.length(), save.sha256());
        var different = new RemoteSaveJournal.Plan(DESTINATION, other);
        assertThrows(IOException.class, () -> RemoteSaveJournal.settle(context, different, BigInteger.ONE));
        assertNull(RemoteSaveJournal.receipt(context, plan));
        RemoteSaveJournal.settle(context, plan, BigInteger.ONE);
        assertThrows(IOException.class, () -> RemoteSaveJournal.receipt(context, different));
    }

    private static File store(Context context) {
        return new File(context.getNoBackupFilesDir(), "remote-saves");
    }

    private static Context context() {
        return InstrumentationRegistry.getInstrumentation().getTargetContext();
    }

    private static RemoteSaveContract.Save save(byte[] bytes) throws Exception {
        String operation = UUID.randomUUID().toString().replace("-", "");
        return new RemoteSaveContract.Save("11".repeat(16), operation, "33".repeat(16), BigInteger.ZERO,
                new RemoteSaveContract.FirstMain("文A.txt"), bytes.length, digest(bytes));
    }

    private static String digest(byte[] bytes) throws Exception {
        return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
    }
}
