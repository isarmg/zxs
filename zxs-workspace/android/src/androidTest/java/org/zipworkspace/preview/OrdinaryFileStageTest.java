package org.zipworkspace.preview;

import android.os.CancellationSignal;
import android.os.OperationCanceledException;
import androidx.test.platform.app.InstrumentationRegistry;
import org.junit.Test;

import java.io.File;
import java.math.BigInteger;
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
