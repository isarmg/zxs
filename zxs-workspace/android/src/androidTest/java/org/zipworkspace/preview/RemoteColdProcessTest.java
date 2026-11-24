package org.zipworkspace.preview;

import android.content.Context;
import androidx.test.platform.app.InstrumentationRegistry;

import org.junit.Test;

import java.io.IOException;
import java.io.InputStream;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Map;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;

public final class RemoteColdProcessTest {
    private static final RemoteSaveJournal.Authority AUTHORITY =
            new RemoteSaveJournal.Authority("55".repeat(32), "alice");
    private static final byte[] BODY = "完整正文".getBytes(StandardCharsets.UTF_8);

    @Test
    public void freezeAndLoseReply() throws Exception {
        var fake = fixture();
        RemoteSaveJournal.prepare(fake.context, AUTHORITY, fake.save, BODY);
        assertEquals(RemoteSaveSubmit.Status.UNCONFIRMED, fake.run(RemoteSaveSubmit.Mode.FIRST_SEND).status());
        assertEquals(1, fake.gets);
        assertEquals(1, fake.posts);
        assertNotNull(RemoteSaveJournal.attempted(fake.context, AUTHORITY, fake.save.operation()));
    }

    @Test
    public void recoverInNewProcess() throws Exception {
        var fake = fixture();
        fake.confirmed = true;
        assertEquals(new RemoteSaveSubmit.Result(RemoteSaveSubmit.Status.CONFIRMED, BigInteger.ONE),
                fake.run(RemoteSaveSubmit.Mode.LOOKUP_ONLY));
        assertEquals(1, fake.gets);
        assertEquals(0, fake.posts);
    }

    @Test
    public void receiptInNewProcessNeedsNoNetwork() throws Exception {
        var fake = fixture();
        assertEquals(new RemoteSaveSubmit.Result(RemoteSaveSubmit.Status.CONFIRMED, BigInteger.ONE),
                fake.run(RemoteSaveSubmit.Mode.LOOKUP_ONLY));
        assertEquals(0, fake.gets);
        assertEquals(0, fake.posts);
    }

    private static Fake fixture() throws Exception {
        Context context = InstrumentationRegistry.getInstrumentation().getTargetContext();
        String operation = InstrumentationRegistry.getArguments().getString("operation");
        if (operation == null || !operation.matches("[0-9a-f]{32}")) {
            throw new IllegalArgumentException("cold fixture needs a unique operation argument");
        }
        var save = new RemoteSaveContract.Save("11".repeat(16), operation, "33".repeat(16),
                BigInteger.ZERO, new RemoteSaveContract.FirstMain("文件.txt"), BODY.length,
                HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(BODY)));
        return new Fake(context, save);
    }

    private static final class Fake implements RemoteSaveSubmit.AuthenticatedExchange {
        final Context context;
        final RemoteSaveContract.Save save;
        boolean confirmed;
        int gets, posts;

        Fake(Context context, RemoteSaveContract.Save save) {
            this.context = context;
            this.save = save;
        }

        RemoteSaveSubmit.Result run(RemoteSaveSubmit.Mode mode) {
            return RemoteSaveSubmit.execute(context, new RemoteSaveJournal.Plan(AUTHORITY, save), mode, this);
        }

        @Override public RemoteSaveJournal.Authority authority() { return AUTHORITY; }

        @Override public RemoteSaveSubmit.Response lookup(String path) {
            gets++;
            assertEquals(save.lookupPath(), path);
            if (confirmed) return receipt();
            return response(404, "{\"state\":\"not_found\"}");
        }

        @Override public RemoteSaveSubmit.Response post(String path, Map<String, String> headers,
                InputStream body) throws IOException {
            posts++;
            assertNotNull(RemoteSaveJournal.attempted(context, AUTHORITY, save.operation()));
            assertEquals(save.submitPath(), path);
            assertEquals(save.submitHeaders(), headers);
            assertArrayEquals(BODY, body.readAllBytes());
            throw new IOException("lost reply");
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
