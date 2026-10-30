package parity.rng;

import java.io.BufferedWriter;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Writes one line per primitive draw, {@code <owner>\t<method>\t<arg>\t<result>}, in draw order.
 * Floats and doubles are written as raw bits ({@code 0x} plus 8 or 16 lowercase hex digits) so
 * both servers' logs compare exactly. Does nothing without {@code -Dparity.rng.log}.
 */
final class DrawLog {
    static final String RESET_MARK = "#reset";
    private static final BufferedWriter OUT = open();

    private DrawLog() {}

    private static BufferedWriter open() {
        String path = System.getProperty("parity.rng.log");
        if (path == null) {
            return null;
        }
        try {
            BufferedWriter out = Files.newBufferedWriter(Path.of(path), StandardCharsets.UTF_8);
            Runtime.getRuntime().addShutdownHook(new Thread(DrawLog::flush, "parity-rng-flush"));
            Thread flusher = new Thread(() -> {
                while (true) {
                    try {
                        Thread.sleep(1000);
                    } catch (InterruptedException e) {
                        return;
                    }
                    flush();
                }
            }, "parity-rng-flusher");
            flusher.setDaemon(true);
            flusher.start();
            return out;
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    static void write(String owner, String method, String arg, String result) {
        if (OUT == null) {
            return;
        }
        synchronized (OUT) {
            try {
                OUT.write(owner + '\t' + method + '\t' + arg + '\t' + result + '\n');
            } catch (IOException ignored) {
                // A failed log write must not change game behaviour.
            }
        }
    }

    /** Marks where every stream restarted; the diff only compares draws after the last mark. */
    static void reset() {
        if (OUT == null) {
            return;
        }
        synchronized (OUT) {
            try {
                OUT.write(RESET_MARK + '\n');
            } catch (IOException ignored) {
                // A failed log write must not change game behaviour.
            }
        }
    }

    static void flush() {
        if (OUT == null) {
            return;
        }
        synchronized (OUT) {
            try {
                OUT.flush();
            } catch (IOException ignored) {
            }
        }
    }

    static String hex(float value) {
        return String.format("0x%08x", Float.floatToRawIntBits(value));
    }

    static String hex(double value) {
        return String.format("0x%016x", Double.doubleToRawLongBits(value));
    }
}
