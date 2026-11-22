package org.zipworkspace.preview;

import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import androidx.test.platform.app.InstrumentationRegistry;
import java.io.File;
import java.io.FileDescriptor;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.Test;
import static org.junit.Assert.*;

/** Real Android syscall contract, confined to a fresh synthetic app-private directory. */
public final class LocalWorkspacePublicationTest {
    private static final int DIRECTORY = OsConstants.O_RDONLY | OsConstants.O_DIRECTORY
            | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC;
    private static final int EXCLUSIVE = OsConstants.O_RDWR | OsConstants.O_CREAT | OsConstants.O_EXCL
            | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC;
    private static final int READ = OsConstants.O_RDONLY | OsConstants.O_NOFOLLOW | OsConstants.O_CLOEXEC;

    @Test public void appPrivateLinkClaimsOneNameForTheSameOriginalInode() throws Exception {
        File root = InstrumentationRegistry.getInstrumentation().getTargetContext().getNoBackupFilesDir();
        Path sample = Files.createTempDirectory(root.toPath(), "publication-proof-");
        String stage = sample.resolve("stage").toString(), slot = sample.resolve("slot").toString();
        String occupied = sample.resolve("occupied").toString(), alias = sample.resolve("alias").toString();
        try {
            FileDescriptor dir = Os.open(sample.toString(), DIRECTORY, 0);
            try {
                FileDescriptor original = Os.open(stage, EXCLUSIVE, 0600);
                try {
                    byte[] data = {1, 2, 3};
                    assertEquals(data.length, Os.pwrite(original, data, 0, data.length, 0));
                    Os.fsync(original);
                    StructStat first = Os.fstat(original);
                    assertEquals(1L, first.st_nlink);

                    FileDescriptor previous = Os.open(occupied, EXCLUSIVE, 0600);
                    try {
                        byte[] sentinel = {9};
                        assertEquals(1, Os.pwrite(previous, sentinel, 0, 1, 0));
                        StructStat prior = Os.fstat(previous);
                        assertEquals(OsConstants.EEXIST,
                                assertThrows(ErrnoException.class, () -> Os.link(stage, occupied)).errno);
                        assertEquals(prior.st_dev, Os.lstat(occupied).st_dev);
                        assertEquals(prior.st_ino, Os.lstat(occupied).st_ino);
                        assertEquals(1L, Os.fstat(original).st_nlink);
                        byte[] unchanged = new byte[1];
                        assertEquals(1, Os.pread(previous, unchanged, 0, 1, 0));
                        assertArrayEquals(sentinel, unchanged);
                    } finally { Os.close(previous); }

                    Os.link(stage, slot);
                    assertEquals(2L, Os.fstat(original).st_nlink);
                    assertEquals(first.st_ino, Os.lstat(slot).st_ino);
                    Os.unlink(stage);
                    Os.fsync(original);
                    Os.fsync(dir);

                    FileDescriptor result = Os.open(slot, READ, 0);
                    try {
                        StructStat actual = Os.fstat(result);
                        assertEquals(first.st_dev, actual.st_dev);
                        assertEquals(first.st_ino, actual.st_ino);
                        assertEquals(1L, actual.st_nlink);
                        byte[] bytes = new byte[data.length];
                        assertEquals(data.length, Os.pread(result, bytes, 0, bytes.length, 0));
                        assertArrayEquals(data, bytes);
                    } finally { Os.close(result); }

                    Os.symlink(slot, alias);
                    assertEquals(OsConstants.ELOOP,
                            assertThrows(ErrnoException.class, () -> Os.open(alias, READ, 0)).errno);
                } finally { Os.close(original); }
            } finally { Os.close(dir); }
        } finally {
            for (String path : new String[]{alias, stage, occupied, slot}) Files.deleteIfExists(Path.of(path));
            Files.delete(sample);
        }
    }
}
