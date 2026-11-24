package org.zipworkspace.preview;

import org.junit.Test;

import java.io.IOException;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.Map;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;

public final class RemoteContainerReadTest {
    private static final String CONTAINER = "11".repeat(16);
    private static final String OPERATION = "22".repeat(16);
    private static final String NODE = "33".repeat(16);
    private static final String OTHER = "44".repeat(16);
    private static final byte[] BODY = "原字节".getBytes(StandardCharsets.UTF_8);
    private static final String DIGEST = ArchiveIndex.hex(ArchiveIndex.sha256().digest(BODY));

    @Test
    public void exactCatalogMainAndNodeAreBoundToSameAccount() throws Exception {
        var peer = new Fake();
        var catalog = RemoteContainerRead.catalog(peer, peer.authority, null);
        assertEquals(1, catalog.items().size());
        assertNull(catalog.next());
        var selection = RemoteContainerRead.selectMain(peer, peer.authority, catalog.items().get(0));
        assertEquals("Report.txt", selection.item().name());
        assertEquals(NODE, selection.revision().node());
        assertEquals(DIGEST, selection.revision().sha256());
        assertArrayEquals(BODY, selection.readExact(peer));
        assertEquals(3, peer.metadataReads);
        assertEquals(1, peer.bodyReads);
    }

    @Test
    public void editedBodyAppendsOnlyToTheExactSelectedMainHead() throws Exception {
        var peer = new Fake();
        var row = RemoteContainerRead.catalog(peer, peer.authority, null).items().get(0);
        var selection = RemoteContainerRead.selectMain(peer, peer.authority, row);
        assertThrows(IllegalArgumentException.class, () -> selection.changedMain(OPERATION, OTHER, BODY));
        byte[] edited = bytes("修改后的全文");
        assertFalse(selection.differs(ByteBuffer.wrap(BODY)));
        assertTrue(selection.differs(ByteBuffer.wrap(edited)));
        var plan = selection.changedMain(OPERATION, OTHER, edited);
        assertEquals(peer.authority(), plan.authority());
        assertEquals(CONTAINER, plan.save().container());
        assertEquals(BigInteger.TWO, plan.save().baseSequence());
        assertEquals(new RemoteSaveContract.BranchHead("main", NODE), plan.save().target());
        assertEquals(edited.length, plan.save().length());
        assertEquals(ArchiveIndex.hex(ArchiveIndex.sha256().digest(edited)), plan.save().sha256());
    }

    @Test
    public void explicitOlderMainChoiceCreatesNewBranchAtThatExactNode() throws Exception {
        var peer = new Fake();
        peer.showHistory = true;
        var original = RemoteContainerRead.catalog(peer, peer.authority, null).items().get(0);
        var anchor = RemoteContainerRead.selectMain(peer, peer.authority, original);
        var older = RemoteContainerRead.mainHistory(peer, anchor, null).select(1);
        assertFalse(anchor.historical());
        assertTrue(older.historical());
        assertEquals(OTHER, older.revision().node());
        String fork = "66".repeat(16);
        String newNode = "77".repeat(16);
        byte[] edited = bytes("Changed");
        assertThrows(IllegalArgumentException.class, () -> older.changedMain(OPERATION, newNode, edited));
        assertThrows(IllegalArgumentException.class, () -> anchor.changedMain(OPERATION, newNode, fork, edited));
        assertEquals(new RemoteSaveContract.HistoricalMain(fork, OTHER),
                older.changedMain(OPERATION, newNode, fork, edited).save().target());
    }

    @Test
    public void changedOwnerSequenceCannotSelectOrDownloadAnotherRevision() throws Exception {
        var peer = new Fake();
        var original = RemoteContainerRead.catalog(peer, peer.authority, null).items().get(0);
        peer.move = true;
        assertThrows(IOException.class, () -> RemoteContainerRead.selectMain(peer, peer.authority, original));
        assertEquals(2, peer.metadataReads);
        assertEquals(0, peer.bodyReads);
    }

    @Test
    public void sameLengthDifferentBodyDoesNotReachTheEditor() throws Exception {
        var peer = new Fake();
        var original = RemoteContainerRead.catalog(peer, peer.authority, null).items().get(0);
        var selection = RemoteContainerRead.selectMain(peer, peer.authority, original);
        peer.wrongBody = true;
        assertThrows(IOException.class, () -> selection.readExact(peer));
    }

    @Test
    public void unanchoredAndAmbiguousHistoryIsRejected() throws Exception {
        String valid = history();
        assertEquals(NODE, RemoteContainerRead.currentRevision(bytes(valid), NODE, BigInteger.TWO).node());
        assertThrows(IOException.class,
                () -> RemoteContainerRead.currentRevision(bytes(valid.replace(NODE, OTHER)), NODE, BigInteger.TWO));
        assertThrows(IOException.class, () -> RemoteContainerRead.currentRevision(
                bytes(valid.replace("\"sequence\":1", "\"sequence\":1,\"sequence\":1")), NODE, BigInteger.TWO));
        assertThrows(IOException.class, () -> RemoteContainerRead.currentRevision(
                bytes(valid.replace("\"next\":null", "\"next\":\"" + OTHER + "\"")), NODE, BigInteger.TWO));
    }

    private static String history() {
        return "{\"head\":\"" + NODE + "\",\"nodes\":[{\"node_id\":\"" + NODE + "\",\"operation_id\":\""
                + OPERATION + "\",\"parent_node\":null,\"original_branch\":\"main\",\"sequence\":1,\"length\":"
                + BODY.length + ",\"sha256\":\"" + DIGEST + "\"}],\"next\":null}";
    }

    private static byte[] bytes(String value) { return value.getBytes(StandardCharsets.UTF_8); }

    private static final class Fake implements RemoteContainerRead.Reader {
        final RemoteSaveJournal.Authority authority = new RemoteSaveJournal.Authority("55".repeat(32), "alice");
        int metadataReads, bodyReads;
        boolean move, wrongBody, showHistory;
        @Override public RemoteSaveJournal.Authority authority() { return authority; }

        @Override public byte[] metadata(String path, Map<String, String> headers) {
            metadataReads++;
            if (path.equals("/zxs/v1/containers")) {
                assertEquals(Map.of("Zxs-View", "compact"), headers);
                return bytes("{\"items\":[{\"container_id\":\"" + CONTAINER
                        + "\",\"sequence\":2,\"name\":\"Report.txt\"}],\"next_after\":null}");
            }
            String detail = "/zxs/v1/containers/" + CONTAINER;
            if (path.equals(detail)) {
                assertEquals(Map.of("Zxs-Detail", "paged", "Zxs-Detail-Sequence", "2"), headers);
                return bytes("{\"container_id\":\"" + CONTAINER + "\",\"sequence\":" + (move ? "3" : "2")
                        + ",\"name\":\"Report.txt\",\"tags\":[],\"heads\":[{\"branch\":\"main\",\"head\":\""
                        + NODE + "\"}],\"next_after\":null}");
            }
            assertEquals(detail + "/branches/main/nodes", path);
            assertEquals(Map.of("Zxs-History-Head", NODE), headers);
            if (showHistory) {
                String nested = history().replace("\"sequence\":1", "\"sequence\":2")
                        .replace("\"parent_node\":null", "\"parent_node\":\"" + OTHER + "\"");
                String previous = history().substring(history().indexOf("[{") + 1, history().indexOf("]"))
                        .replace(NODE, OTHER);
                return bytes(nested.replace("}],\"next\":null}", "}," + previous + "],\"next\":null}"));
            }
            return bytes(history());
        }

        @Override public byte[] node(String container, String node, int length, String sha256) {
            bodyReads++;
            assertEquals(CONTAINER, container);
            assertEquals(NODE, node);
            assertEquals(BODY.length, length);
            assertEquals(DIGEST, sha256);
            byte[] result = BODY.clone();
            if (wrongBody) result[0] ^= 1;
            return result;
        }
    }
}
