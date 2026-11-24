package org.zipworkspace.preview;

import android.os.CancellationSignal;
import android.os.OperationCanceledException;
import android.system.Os;
import androidx.test.platform.app.InstrumentationRegistry;
import org.junit.Test;

import java.io.File;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.io.IOException;
import java.nio.file.Files;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Set;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertThrows;

public final class OrdinaryFileStageTest {
    private static final class Source implements OrdinaryFileStage.Source {
        final byte[] original = new byte[400_001];
        final CancellationSignal cancel = new CancellationSignal();
        boolean changed, trailing, cancelMidstream;
        Source() {
            for (int i = 0; i < original.length; i++) original[i] = (byte) i;
        }
        @Override public long size() { return original.length; }
        @Override public void validate() throws IOException {
            if (changed) throw new IOException("source changed");
        }
        @Override public void fill(long offset, byte[] bytes, int count) {
            assertTrue(count > 0 && count <= 65_536);
            System.arraycopy(original, (int) offset, bytes, 0, count);
            if (cancelMidstream) cancel.cancel();
        }
        @Override public void checkEnd() throws IOException {
            if (trailing) throw new IOException("source grew");
        }
    }

    @Test public void stagesBinaryAboveTextLimitWithoutChangingSelectedBytes() throws Exception {
        var source = new Source();
        var original = source.original.clone();
        var result = OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), source, source.cancel);
        assertEquals(original.length, result.length());
        assertArrayEquals(original, source.original);
        var body = new File(store(), result.id() + ".body");
        try {
            assertArrayEquals(original, Files.readAllBytes(body.toPath()));
            assertEquals(result, OrdinaryFileStage.reopen(context(), result, source.cancel));
            assertEquals(HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(original)),
                    result.sha256());
        } finally {
            Files.delete(body.toPath());
        }
    }

    @Test public void onlyExactOwnedInterruptedPublicationCanBeRecovered() throws Exception {
        var source = new Source();
        var result = OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), source, source.cancel);
        var body = new File(store(), result.id() + ".body").toPath();
        var pending = new File(store(), result.id() + ".pending").toPath();
        try {
            Files.createLink(pending, body);
            var wrong = new OrdinaryFileStage.Staged(result.id(), result.length(), "0".repeat(64));
            assertThrows(IOException.class, () -> OrdinaryFileStage.reopen(context(), wrong, source.cancel));
            assertTrue(Files.isSameFile(body, pending));
            assertEquals(result, OrdinaryFileStage.reopen(context(), result, source.cancel));
            assertTrue(!Files.exists(pending));
        } finally {
            Files.deleteIfExists(pending);
            Files.deleteIfExists(body);
        }
    }

    @Test public void diskPlanReopensExactVerifiedBodyAndRefusesChangedSource() throws Exception {
        var source = new Source();
        var result = OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), source, source.cancel);
        var path = new File(store(), result.id() + ".body").toPath();
        var authority = new RemoteSaveJournal.Authority("55".repeat(32), "alice");
        var save = new RemoteSaveContract.Save("11".repeat(16), result.id(), "33".repeat(16),
                BigInteger.ZERO, new RemoteSaveContract.FirstMain("whole.bin"), result.length(), result.sha256());
        var plan = new RemoteSaveJournal.Plan(authority, save);
        try {
            RemoteSaveJournal.prepareStaged(context(), plan, result);
            assertEquals(save, RemoteSaveJournal.reopen(context(), authority, result.id()).save());
            assertThrows(IOException.class, () -> RemoteSaveJournal.discardUnboundStage(context(), result));
            assertTrue(Files.exists(path));
            assertTrue(RemoteSaveJournal.plans(context(), authority, null).rows().contains(plan));
            try (var held = RemoteSaveJournal.beginWithBody(context(), plan)) {
                assertArrayEquals(source.original, held.readAllBytes());
            }
            assertEquals(plan, RemoteSaveJournal.attempted(context(), authority, result.id()));
            byte[] corrupt = source.original.clone();
            corrupt[0] ^= 1;
            Files.write(path, corrupt);
            assertThrows(IOException.class, () -> RemoteSaveJournal.beginWithBody(context(), plan));
            assertEquals(plan, RemoteSaveJournal.attempted(context(), authority, result.id()));
        } finally {
            Files.deleteIfExists(path);
        }
    }

    @Test public void canonicalStagedPlanRepairsOnlyItsExactPublicationLink() throws Exception {
        var source = new Source();
        var staged = OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), source, source.cancel);
        var body = new File(store(), staged.id() + ".body").toPath();
        var authority = new RemoteSaveJournal.Authority("55".repeat(32), "alice");
        var save = new RemoteSaveContract.Save("11".repeat(16), staged.id(), "33".repeat(16),
                BigInteger.ZERO, new RemoteSaveContract.FirstMain("body.bin"), staged.length(), staged.sha256());
        var plan = new RemoteSaveJournal.Plan(authority, save);
        var journal = new File(context().getNoBackupFilesDir(), "remote-saves").toPath();
        var published = journal.resolve(staged.id() + ".plan");
        var pending = journal.resolve("." + staged.id() + ".plan.pending");
        var wrong = journal.resolve("." + staged.id() + ".unrelated");
        try {
            RemoteSaveJournal.prepareStaged(context(), plan, staged);
            assertTrue(!Files.exists(journal.resolve(staged.id() + ".save")));

            // Before publication only the candidate exists; do not open, send or remove it.
            Files.move(published, pending);
            assertThrows(IOException.class, () -> RemoteSaveJournal.reopen(context(), authority, staged.id()));
            assertTrue(Files.exists(pending));
            assertTrue(!RemoteSaveJournal.plans(context(), authority, null).rows().contains(plan));

            // Published plus private pending link: discover read-only, then reopen exact marker.
            Files.createLink(published, pending);
            assertTrue(RemoteSaveJournal.plans(context(), authority, null).rows().contains(plan));
            assertTrue(Files.isSameFile(published, pending));
            var different = new RemoteSaveJournal.Authority(authority.destinationIdentity(), "bob");
            assertThrows(IOException.class, () -> RemoteSaveJournal.reopen(context(), different, staged.id()));
            assertTrue(Files.isSameFile(published, pending));
            assertEquals(save, RemoteSaveJournal.reopen(context(), authority, staged.id()).save());
            assertTrue(!Files.exists(pending));

            // A second link at the wrong name plus an unrelated pending inode is not repaired.
            Files.createLink(wrong, published);
            Files.writeString(pending, "unrelated");
            assertThrows(IOException.class, () -> RemoteSaveJournal.reopen(context(), authority, staged.id()));
            assertEquals("unrelated", Files.readString(pending));
            assertTrue(Files.isSameFile(wrong, published));
        } finally {
            Files.deleteIfExists(pending);
            Files.deleteIfExists(wrong);
            Files.deleteIfExists(published);
            Files.deleteIfExists(body);
        }
    }

    @Test public void priorTwoRecordStagedOperationStillReopensWithoutRewrite() throws Exception {
        var source = new Source();
        var staged = OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), source, source.cancel);
        var body = new File(store(), staged.id() + ".body").toPath();
        var authority = new RemoteSaveJournal.Authority("55".repeat(32), "alice");
        var save = new RemoteSaveContract.Save("11".repeat(16), staged.id(), "33".repeat(16),
                BigInteger.ZERO, new RemoteSaveContract.FirstMain("legacy.bin"), staged.length(), staged.sha256());
        var plan = new RemoteSaveJournal.Plan(authority, save);
        var journal = new File(context().getNoBackupFilesDir(), "remote-saves").toPath();
        var selector = journal.resolve(staged.id() + ".plan");
        var binding = journal.resolve(staged.id() + ".save");
        try {
            RemoteSaveJournal.prepareStaged(context(), plan, staged);
            byte[] originalBinding = Files.readAllBytes(selector);
            Files.write(binding, originalBinding);
            Os.chmod(binding.toString(), 0600);

            // Reproduce the earlier canonical selector (RSP1), whose remaining
            // fields are identical to the existing staged body marker (RSF1).
            byte[] oldSelector = originalBinding.clone();
            ByteBuffer.wrap(oldSelector).putInt(0x52535031);
            int seal = oldSelector.length - 32;
            var digest = MessageDigest.getInstance("SHA-256");
            digest.update(oldSelector, 0, seal);
            System.arraycopy(digest.digest(), 0, oldSelector, seal, 32);
            Files.write(selector, oldSelector);

            assertTrue(RemoteSaveJournal.plans(context(), authority, null).rows().contains(plan));
            assertEquals(save, RemoteSaveJournal.reopen(context(), authority, staged.id()).save());
            assertArrayEquals(originalBinding, Files.readAllBytes(binding));
            assertArrayEquals(oldSelector, Files.readAllBytes(selector));
        } finally {
            Files.deleteIfExists(binding);
            Files.deleteIfExists(selector);
            Files.deleteIfExists(body);
        }
    }

    @Test public void unchangedReplacementPublishesNothing() throws Exception {
        var before = names();
        var source = new Source();
        String digest = HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(source.original));
        var original = new OrdinaryFileStage.Baseline(source.original.length, digest);
        assertThrows(OrdinaryFileStage.Unchanged.class, () -> OrdinaryFileStage.stage(context(),
                OrdinaryFileStage.newId(), source, source.cancel, original));
        assertEquals(before, names());
    }

    @Test public void unboundCancellationDiscardsOnlyItsOwnedBody() throws Exception {
        var source = new Source();
        var first = OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), source, source.cancel);
        var second = OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), source, source.cancel);
        try {
            source.cancel.cancel();
            assertThrows(OperationCanceledException.class, source.cancel::throwIfCanceled);
            RemoteSaveJournal.discardUnboundStage(context(), first);
            assertTrue(!names().contains(first.id() + ".body"));
            assertTrue(names().contains(second.id() + ".body"));
        } finally {
            Files.deleteIfExists(new File(store(), first.id() + ".body").toPath());
            Files.deleteIfExists(new File(store(), second.id() + ".body").toPath());
        }
    }

    @Test public void changedTrailingOrCancelledSourcePublishesNothing() throws Exception {
        var before = names();
        var changed = new Source();
        changed.changed = true;
        assertThrows(IOException.class, () -> OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), changed, changed.cancel));
        var trailing = new Source();
        trailing.trailing = true;
        assertThrows(IOException.class, () -> OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), trailing, trailing.cancel));
        var cancelled = new Source();
        cancelled.cancelMidstream = true;
        assertThrows(OperationCanceledException.class,
                () -> OrdinaryFileStage.stage(context(), OrdinaryFileStage.newId(), cancelled, cancelled.cancel));
        assertEquals(before, names());
    }

    private static Set<String> names() {
        var store = store();
        if (!store.exists()) return Set.of();
        String[] names = store.list();
        assertNotNull(names);
        return Set.of(names);
    }

    private static File store() { return new File(context().getNoBackupFilesDir(), "ordinary-bodies"); }

    private static android.content.Context context() {
        return InstrumentationRegistry.getInstrumentation().getTargetContext();
    }
}
