package org.zipworkspace.preview;

import org.junit.Test;

import java.math.BigInteger;
import java.nio.charset.StandardCharsets;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;

public final class RemoteSaveContractTest {
    private static final String CONTAINER = "11".repeat(16);
    private static final String OPERATION = "22".repeat(16);
    private static final String NODE = "33".repeat(16);
    private static final String ORIGINAL = "44".repeat(16);
    private static final String BRANCH = "55".repeat(16);
    private static final String EMPTY_SHA = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    @Test
    public void firstSaveUsesExactCaseAndWholeBodyHeaders() {
        var save = save(new RemoteSaveContract.FirstMain("文A.txt"), BigInteger.ZERO);
        assertEquals("/zxs/v1/containers/" + CONTAINER + "/operations/" + OPERATION, save.lookupPath());
        assertEquals("/zxs/v1/containers/" + CONTAINER + "/branches/main/nodes", save.submitPath());
        assertEquals("e69687412e747874", save.submitHeaders().get("Zxs-Initial-Name"));
        assertEquals("0", save.submitHeaders().get("Zxs-Base-Node"));
        assertEquals("0", save.submitHeaders().get("Content-Length"));
        assertEquals(EMPTY_SHA, save.submitHeaders().get("Zxs-Sha256"));
        assertEquals(OPERATION, save.submitHeaders().get("Zxs-Operation"));
        assertEquals(NODE, save.submitHeaders().get("Zxs-Node"));
        assertFalse(save.submitHeaders().containsKey("Zxs-Fork-From-Main"));
    }

    @Test
    public void appendAndHistoricalForkKeepSeparateOriginalSelectors() {
        var head = save(new RemoteSaveContract.BranchHead("main", ORIGINAL), BigInteger.ONE);
        var fork = save(new RemoteSaveContract.HistoricalMain(BRANCH, ORIGINAL), BigInteger.ONE);
        assertEquals(ORIGINAL, head.submitHeaders().get("Zxs-Base-Node"));
        assertFalse(head.submitHeaders().containsKey("Zxs-Fork-From-Main"));
        assertFalse(head.submitHeaders().containsKey("Zxs-Initial-Name"));
        assertEquals(ORIGINAL, fork.submitHeaders().get("Zxs-Fork-From-Main"));
        assertFalse(fork.submitHeaders().containsKey("Zxs-Base-Node"));
        assertEquals("/zxs/v1/containers/" + CONTAINER + "/branches/" + BRANCH + "/nodes",
                fork.submitPath());
        String forkTarget = "{\"kind\":\"historical_main\",\"branch\":\"" + BRANCH
                + "\",\"from_node\":\"" + ORIGINAL + "\"}";
        var result = reply(fork, true, 200, receipt("2", forkTarget));
        assertEquals(RemoteSaveContract.Outcome.CONFIRMED, result.outcome());
        assertEquals(BigInteger.TWO, result.sequence());
        assertEquals(RemoteSaveContract.Outcome.UNKNOWN,
                reply(head, true, 200, receipt("2", forkTarget)).outcome());
    }

    @Test
    public void absenceRequiresExactLookupResponse() {
        var save = save(new RemoteSaveContract.FirstMain("a"), BigInteger.ZERO);
        String absent = "{\"state\":\"not_found\"}";
        assertEquals(RemoteSaveContract.Outcome.ABSENT, reply(save, true, 404, absent).outcome());
        assertEquals(RemoteSaveContract.Outcome.UNKNOWN, reply(save, false, 404, absent).outcome());
        assertEquals(RemoteSaveContract.Outcome.UNKNOWN,
                reply(save, true, 404, "{\"state\":\"not_found\",\"state\":\"not_found\"}").outcome());
        assertEquals(RemoteSaveContract.Outcome.UNKNOWN, reply(save, true, 404, "{}").outcome());
        assertEquals(RemoteSaveContract.Outcome.CONFLICT, reply(save, false, 409, "").outcome());
    }

    @Test
    public void mismatchedOrAmbiguousReceiptNeverConfirms() {
        var save = save(new RemoteSaveContract.FirstMain("A"), BigInteger.ZERO);
        String target = "{\"kind\":\"first_main\",\"name\":\"A\"}";
        String valid = receipt("1", target);
        assertEquals(RemoteSaveContract.Outcome.CONFIRMED, reply(save, false, 200, valid).outcome());
        for (String rejected : new String[] {
                receipt("0", target),
                receipt("\"1\"", target),
                receipt("1e0", target),
                receipt("18446744073709551615", target),
                receipt("18446744073709551616", target),
                receipt("1", target.replace("\"A\"", "\"a\"")),
                receipt("1", target.replace("\"name\"", "\"unexpected\"")),
                receipt("1", target.replace("\"name\":\"A\"", "\"name\":\"wrong\",\"name\":\"A\"")),
                valid.replace(OPERATION, ORIGINAL),
                valid.replace(NODE, ORIGINAL),
                valid.replace(EMPTY_SHA, "0".repeat(64)),
                valid.replace("\"length\":0", "\"length\":1"),
                valid + " {}",
                valid + " ".repeat(16 * 1024),
        }) {
            assertEquals(rejected.substring(0, Math.min(270, rejected.length())),
                    RemoteSaveContract.Outcome.UNKNOWN, reply(save, true, 200, rejected).outcome());
        }
        assertEquals(RemoteSaveContract.Outcome.UNKNOWN,
                RemoteSaveContract.project(save, true, 200, new byte[] {(byte) 0xC0, (byte) 0xAF}).outcome());
    }

    @Test
    public void initialAndSubsequentSaveMustUseTheirOriginalBase() {
        assertThrows(IllegalArgumentException.class, () ->
                save(new RemoteSaveContract.FirstMain("a"), BigInteger.ONE));
        assertThrows(IllegalArgumentException.class, () ->
                save(new RemoteSaveContract.BranchHead("main", ORIGINAL), BigInteger.ZERO));
        assertThrows(IllegalArgumentException.class, () ->
                save(new RemoteSaveContract.BranchHead("main", NODE), BigInteger.ONE));
        assertThrows(IllegalArgumentException.class, () -> new RemoteSaveContract.HistoricalMain("main", ORIGINAL));
        for (String invalid : new String[] {
                "../a", "a.", "a ", "a<b", "a>b", "a:b", "a\"b", "a\\b", "a|b", "a?b", "a*b",
                "a\u0085b", "\uD800", "文".repeat(86),
        }) {
            assertThrows(IllegalArgumentException.class, () -> new RemoteSaveContract.FirstMain(invalid));
        }
        assertEquals("文".repeat(85), new RemoteSaveContract.FirstMain("文".repeat(85)).name());
    }

    private static RemoteSaveContract.Save save(RemoteSaveContract.Target target, BigInteger base) {
        return new RemoteSaveContract.Save(CONTAINER, OPERATION, NODE, base, target, 0, EMPTY_SHA);
    }

    private static RemoteSaveContract.Reply reply(RemoteSaveContract.Save save, boolean lookup,
                                                  int status, String text) {
        return RemoteSaveContract.project(save, lookup, status, text.getBytes(StandardCharsets.UTF_8));
    }

    private static String receipt(String sequence, String target) {
        return "{\"kind\":\"save_node\",\"container_id\":\"" + CONTAINER + "\",\"operation_id\":\""
                + OPERATION + "\",\"sequence\":" + sequence + ",\"node_id\":\"" + NODE
                + "\",\"length\":0,\"sha256\":\"" + EMPTY_SHA + "\",\"target\":" + target + "}";
    }
}
