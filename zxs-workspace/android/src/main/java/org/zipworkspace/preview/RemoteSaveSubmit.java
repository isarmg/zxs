package org.zipworkspace.preview;

import android.content.Context;

import java.io.IOException;
import java.io.InputStream;
import java.math.BigInteger;
import java.util.Map;
import java.util.Objects;

/** One explicitly requested Android R3 SAVE operation; no automatic network replays. */
final class RemoteSaveSubmit {
    enum Mode { LOOKUP_ONLY, FIRST_SEND, RETRY_SAME_OPERATION }

    enum Status {
        CONFIRMED, ABSENT, RETRY_REQUIRES_CONFIRMATION,
        CONFLICT, UNCONFIRMED, LOCAL_UNAVAILABLE, LOCAL_RECEIPT_UNCONFIRMED
    }

    record Result(Status status, BigInteger sequence) {}

    /** Frames must be fully read, bounded and media/framing-checked before this return. */
    record Response(int status, byte[] data) {
        Response {
            Objects.requireNonNull(data);
        }
    }

    interface AuthenticatedExchange {
        /** Immutable binding to the selected HTTPS/TLS policy and actual account for every request. */
        RemoteSaveJournal.Authority authority();

        /** Open a fresh verified HTTP/2 request. Never redirect, fall back or retry. */
        Response lookup(String exactOriginPath) throws IOException;

        /** Send exactly the declared whole body or return no settlement proof. */
        Response post(String exactOriginPath, Map<String, String> headers, InputStream completeBody)
                throws IOException;
    }

    private RemoteSaveSubmit() {}

    /**
     * Serialization is within this single app process. Call on a bounded-time worker,
     * never the UI thread. No process or service other than this app owns the journal.
     */
    static synchronized Result execute(Context context, RemoteSaveJournal.Plan selected,
                                       Mode mode, AuthenticatedExchange exchange) {
        if (selected == null || mode == null || exchange == null
                || !selected.authority().equals(exchange.authority())) {
            return status(Status.UNCONFIRMED);
        }

        var authority = selected.authority();
        String operation = selected.save().operation();
        RemoteSaveJournal.Plan attempted;
        RemoteSaveJournal.Plan original;
        try {
            attempted = RemoteSaveJournal.attempted(context, authority, operation);
            original = attempted;
            if (original == null) {
                var prepared = RemoteSaveJournal.reopen(context, authority, operation);
                original = new RemoteSaveJournal.Plan(prepared.authority(), prepared.save());
            }
            if (!selected.equals(original)) return status(Status.LOCAL_UNAVAILABLE);
            BigInteger alreadyRecorded = RemoteSaveJournal.receipt(context, original);
            if (alreadyRecorded != null) return new Result(Status.CONFIRMED, alreadyRecorded);
        } catch (IOException unavailable) {
            return status(Status.LOCAL_UNAVAILABLE);
        }

        var save = original.save();
        RemoteSaveContract.Reply before;
        try {
            var response = exchange.lookup(save.lookupPath());
            if (!authority.equals(exchange.authority()) || response == null) return status(Status.UNCONFIRMED);
            before = RemoteSaveContract.project(save, true, response.status(), response.data());
        } catch (IOException absentProof) {
            return status(Status.UNCONFIRMED);
        }
        if (before.outcome() == RemoteSaveContract.Outcome.CONFIRMED) {
            // An unexpected existing exact receipt must not authorize a send.
            // Without our own attempt marker, it is not a durable local settlement.
            return attempted == null ? status(Status.LOCAL_RECEIPT_UNCONFIRMED)
                    : settle(context, original, before.sequence());
        }
        if (before.outcome() == RemoteSaveContract.Outcome.CONFLICT) return status(Status.CONFLICT);
        if (before.outcome() != RemoteSaveContract.Outcome.ABSENT) return status(Status.UNCONFIRMED);

        if (mode == Mode.LOOKUP_ONLY) return status(Status.ABSENT);
        if (mode == Mode.FIRST_SEND && attempted != null) {
            return status(Status.RETRY_REQUIRES_CONFIRMATION);
        }
        if (mode == Mode.RETRY_SAME_OPERATION && attempted == null) {
            return status(Status.ABSENT);
        }

        if (Thread.currentThread().isInterrupted()) return status(Status.UNCONFIRMED);
        InputStream held;
        try {
            // Prove the original body after lookup, then hold it while persisting the attempt.
            held = RemoteSaveJournal.beginWithBody(context, original);
        } catch (IOException unavailable) {
            return status(Status.LOCAL_UNAVAILABLE);
        }

        try (InputStream body = held) {
            if (Thread.currentThread().isInterrupted() || !authority.equals(exchange.authority())) {
                return status(Status.UNCONFIRMED);
            }
            var response = exchange.post(save.submitPath(), save.submitHeaders(), body);
            if (!authority.equals(exchange.authority()) || response == null) return status(Status.UNCONFIRMED);
            var after = RemoteSaveContract.project(save, false, response.status(), response.data());
            if (after.outcome() == RemoteSaveContract.Outcome.CONFIRMED) {
                return settle(context, original, after.sequence());
            }
            if (after.outcome() == RemoteSaveContract.Outcome.CONFLICT) return status(Status.CONFLICT);
            return status(Status.UNCONFIRMED);
        } catch (IOException lost) {
            return status(Status.UNCONFIRMED);
        }
    }

    private static Result settle(Context context, RemoteSaveJournal.Plan original, BigInteger sequence) {
        try {
            RemoteSaveJournal.settle(context, original, sequence);
            return new Result(Status.CONFIRMED, sequence);
        } catch (IOException unavailable) {
            return status(Status.LOCAL_RECEIPT_UNCONFIRMED);
        }
    }

    private static Result status(Status status) {
        return new Result(status, null);
    }
}
