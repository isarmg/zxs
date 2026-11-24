package org.zipworkspace.preview;

import android.content.Context;
import androidx.test.platform.app.InstrumentationRegistry;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.RandomAccessFile;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Map;
import java.util.UUID;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;

public final class RemoteSaveSubmitTest {
    private static final RemoteSaveJournal.Authority AUTHORITY =
            new RemoteSaveJournal.Authority("55".repeat(32), "alice");
    private static final byte[] BODY = "完整正文".getBytes(StandardCharsets.UTF_8);

    @Test
    public void lostReplyQueriesOriginalOperationWithoutTheBody() throws Exception {
        var fake = prepare();
        fake.loseReply = true;
        assertEquals(RemoteSaveSubmit.Status.UNCONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(1, fake.posts);
        fake.confirmed = true;
        try (var body = new RandomAccessFile(fake.frozenFile(), "rw")) {
            body.setLength(0);
        }
        assertEquals(new RemoteSaveSubmit.Result(RemoteSaveSubmit.Status.CONFIRMED, BigInteger.ONE),
                fake.run(RemoteSaveSubmit.Mode.LOOKUP_ONLY));
        assertEquals(2, fake.gets);
        assertEquals(1, fake.posts);
    }

    @Test
    public void retryRequiresDistinctUserChoiceAndFreshExactLookup() throws Exception {
        var fake = prepare();
        fake.loseReply = true;
        assertEquals(RemoteSaveSubmit.Status.UNCONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(RemoteSaveSubmit.Status.RETRY_REQUIRES_CONFIRMATION,
                fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(RemoteSaveSubmit.Status.ABSENT,
                fake.run(RemoteSaveSubmit.Mode.LOOKUP_ONLY).status());
        assertEquals(1, fake.posts);
        fake.unverifiedMissing = true;
        assertEquals(RemoteSaveSubmit.Status.UNCONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.RETRY_SAME_OPERATION).status());
        assertEquals(1, fake.posts);
        fake.unverifiedMissing = false;
        fake.loseReply = false;
        assertEquals(RemoteSaveSubmit.Status.CONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.RETRY_SAME_OPERATION).status());
        assertEquals(5, fake.gets);
        assertEquals(2, fake.posts);
    }

    @Test
    public void missingOrChangedLocalProofCannotSendAfterLookup() throws Exception {
        var fake = prepare();
        fake.damageOnLookup = true;
        assertEquals(RemoteSaveSubmit.Status.LOCAL_UNAVAILABLE,
                fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(1, fake.gets);
        assertEquals(0, fake.posts);
    }

    @Test
    public void differentAccountCannotUseOriginalDraft() throws Exception {
        var fake = prepare();
        fake.peer = new RemoteSaveJournal.Authority(AUTHORITY.destinationIdentity(), "Alice");
        assertEquals(RemoteSaveSubmit.Status.UNCONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(0, fake.gets);
        assertEquals(0, fake.posts);
    }

    @Test
    public void unexpectedReceiptDoesNotForgeAnAttemptOrSend() throws Exception {
        var fake = prepare();
        fake.confirmed = true;
        assertEquals(RemoteSaveSubmit.Status.LOCAL_RECEIPT_UNCONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(1, fake.gets);
        assertEquals(0, fake.posts);
        assertNull(RemoteSaveJournal.attempted(fake.context, AUTHORITY, fake.save.operation()));
    }

    @Test
    public void changedAuthorityDuringLookupCannotSettleEvenAnExactReceipt() throws Exception {
        var fake = prepare();
        var retained = RemoteSaveJournal.reopen(fake.context, AUTHORITY, fake.save.operation());
        RemoteSaveJournal.beginAttempt(fake.context, retained);
        fake.confirmed = true;
        fake.switchDuringLookup = true;
        assertEquals(RemoteSaveSubmit.Status.UNCONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.LOOKUP_ONLY).status());
        assertEquals(1, fake.gets);
        assertEquals(0, fake.posts);
        assertNull(RemoteSaveJournal.receipt(fake.context, new RemoteSaveJournal.Plan(AUTHORITY, fake.save)));
    }

    @Test
    public void changedAuthorityDuringPostCannotSettleEvenAnExactReceipt() throws Exception {
        var fake = prepare();
        fake.switchDuringPost = true;
        assertEquals(RemoteSaveSubmit.Status.UNCONFIRMED,
                fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(1, fake.gets);
        assertEquals(1, fake.posts);
        assertNotNull(RemoteSaveJournal.attempted(fake.context, AUTHORITY, fake.save.operation()));
        assertNull(RemoteSaveJournal.receipt(fake.context, new RemoteSaveJournal.Plan(AUTHORITY, fake.save)));
    }

    @Test
    public void previouslyConfirmedPlanCannotSilentlyChooseDifferentOriginal() throws Exception {
        var fake = prepare();
        var save = fake.save;
        var changed = new RemoteSaveContract.Save(save.container(), save.operation(), save.node(),
                save.baseSequence(), new RemoteSaveContract.FirstMain("another.txt"), save.length(), save.sha256());
        var selected = new RemoteSaveJournal.Plan(AUTHORITY, changed);
        assertEquals(RemoteSaveSubmit.Status.LOCAL_UNAVAILABLE, RemoteSaveSubmit.execute(
                fake.context, selected, RemoteSaveSubmit.Mode.FIRST_SEND, fake).status());
        assertEquals(0, fake.gets);
        assertEquals(0, fake.posts);
    }

    private static Fake prepare() throws Exception {
        Context context = InstrumentationRegistry.getInstrumentation().getTargetContext();
        String op = UUID.randomUUID().toString().replace("-", "");
        var save = new RemoteSaveContract.Save("11".repeat(16), op, "33".repeat(16), BigInteger.ZERO,
                new RemoteSaveContract.FirstMain("文件.txt"), BODY.length,
                HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(BODY)));
        RemoteSaveJournal.prepare(context, AUTHORITY, save, BODY);
        return new Fake(context, save);
    }

    private static final class Fake implements RemoteSaveSubmit.AuthenticatedExchange {
        final Context context;
        final RemoteSaveContract.Save save;
        RemoteSaveJournal.Authority peer = AUTHORITY;
        boolean confirmed, loseReply, unverifiedMissing, damageOnLookup, switchDuringLookup, switchDuringPost;
        int gets, posts;

        Fake(Context context, RemoteSaveContract.Save save) {
            this.context = context;
            this.save = save;
        }

        File frozenFile() {
            return new File(new File(context.getNoBackupFilesDir(), "remote-saves"),
                    save.operation() + ".save");
        }

        RemoteSaveSubmit.Result run(RemoteSaveSubmit.Mode mode) {
            return RemoteSaveSubmit.execute(context, new RemoteSaveJournal.Plan(AUTHORITY, save), mode, this);
        }

        @Override public RemoteSaveJournal.Authority authority() { return peer; }

        @Override
        public RemoteSaveSubmit.Response lookup(String path) throws IOException {
            gets++;
            assertEquals(save.lookupPath(), path);
            if (damageOnLookup) {
                try (var body = new RandomAccessFile(frozenFile(), "rw")) {
                    body.setLength(0);
                }
            }
            if (switchDuringLookup) changeAuthority();
            if (confirmed) return receipt();
            return response(404, unverifiedMissing ? "{}" : "{\"state\":\"not_found\"}");
        }

        @Override
        public RemoteSaveSubmit.Response post(String path, Map<String, String> headers, InputStream body)
                throws IOException {
            posts++;
            assertNotNull(RemoteSaveJournal.attempted(context, AUTHORITY, save.operation()));
            assertEquals(save.submitPath(), path);
            assertEquals(save.submitHeaders(), headers);
            assertArrayEquals(BODY, body.readAllBytes());
            if (switchDuringPost) changeAuthority();
            if (loseReply) throw new IOException("lost reply");
            return receipt();
        }

        private void changeAuthority() {
            peer = new RemoteSaveJournal.Authority(AUTHORITY.destinationIdentity(), "bob");
        }

        private RemoteSaveSubmit.Response receipt() {
            return response(200, "{\"kind\":\"save_node\",\"container_id\":\"" + save.container()
                    + "\",\"operation_id\":\"" + save.operation() + "\",\"sequence\":1,\"node_id\":\""
                    + save.node() + "\",\"length\":" + save.length() + ",\"sha256\":\"" + save.sha256()
                    + "\",\"target\":{\"kind\":\"first_main\",\"name\":\"文件.txt\"}}");
        }

        private static RemoteSaveSubmit.Response response(int status, String raw) {
            return new RemoteSaveSubmit.Response(status, raw.getBytes(StandardCharsets.UTF_8));
        }
    }
}
