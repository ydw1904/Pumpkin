package parity.rng;

import java.util.ArrayDeque;
import java.util.HashMap;
import java.util.Map;
import java.util.WeakHashMap;
import java.util.concurrent.ConcurrentHashMap;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

/**
 * Keeps the current owner of gameplay randomness and one Java-LCG stream per owner.
 *
 * <p>Owner keys (the Pumpkin side builds the same ones):
 * <ul>
 *   <li>{@code <type path>@<x>,<y>,<z>#<n>}: an entity, by its block position when registered and
 *       the number of earlier registrations with the same type and position.</li>
 *   <li>{@code player:<name>}: a player.</li>
 *   <li>{@code block@x,y,z} / {@code fluid@x,y,z}: a scheduled block or fluid tick.</li>
 *   <li>{@code chunk@cx,cz}: a chunk tick (lightning, natural spawning, weather, random ticks).</li>
 *   <li>{@code chunks}: ticking the chunk list (the spawning-chunk shuffle, custom spawners).</li>
 *   <li>{@code misc}: everything else.</li>
 * </ul>
 * Stream for key K: {@code new LegacyRandomSource(seed ^ (long) K.hashCode() ^ ((long) epoch << 32))},
 * where epoch counts {@code /tick freeze} resets (see {@link #setFrozen}).
 */
public final class Router {
    static final long SEED = Long.parseLong(System.getProperty("parity.rng.seed"));
    private static final String MISC = "misc";
    private static final ThreadLocal<ArrayDeque<String>> OWNERS = ThreadLocal.withInitial(ArrayDeque::new);
    private static final Map<String, LegacyRandomSource> STREAMS = new ConcurrentHashMap<>();
    private static final Map<Entity, String> KEYS = new WeakHashMap<>();
    private static final Map<String, Integer> REGISTRATIONS = new HashMap<>();
    // Resets so far. Mixed into stream seeds so a restarted stream doesn't replay values drawn
    // before the reset (vanilla refuses an entity whose UUID already exists).
    private static volatile int epoch;

    private Router() {}

    /** Replaces {@code RandomSource.create()} for {@code Entity.random} and {@code Level.random}. */
    public static RandomSource routedRandom() {
        return new RoutedRandom();
    }

    public static void register(Entity entity) {
        keyOf(entity);
    }

    public static void enterEntity(Entity entity) {
        OWNERS.get().push(keyOf(entity));
    }

    public static void enterBlock(BlockPos pos) {
        OWNERS.get().push("block@" + pos.getX() + "," + pos.getY() + "," + pos.getZ());
    }

    public static void enterFluid(BlockPos pos) {
        OWNERS.get().push("fluid@" + pos.getX() + "," + pos.getY() + "," + pos.getZ());
    }

    public static void enterChunk(LevelChunk chunk) {
        ChunkPos pos = chunk.getPos();
        OWNERS.get().push("chunk@" + pos.x() + "," + pos.z());
    }

    /**
     * Hooked into {@code ServerTickRateManager.setFrozen}. Freezing restarts every stream and the
     * entity numbering, so a scene built while frozen starts from the same state on both servers.
     */
    public static void setFrozen(boolean frozen) {
        if (!frozen) {
            return;
        }
        synchronized (Router.class) {
            epoch++;
            STREAMS.clear();
            REGISTRATIONS.clear();
        }
        DrawLog.reset();
    }

    public static void enterChunks() {
        OWNERS.get().push("chunks");
    }

    public static void exit() {
        OWNERS.get().poll();
    }

    static String owner() {
        String owner = OWNERS.get().peek();
        return owner == null ? MISC : owner;
    }

    /** The current owner's stream; callers synchronize on it. */
    static LegacyRandomSource stream(String owner) {
        return STREAMS.computeIfAbsent(
            owner, key -> new LegacyRandomSource(SEED ^ (long) key.hashCode() ^ ((long) epoch << 32)));
    }

    private static synchronized String keyOf(Entity entity) {
        String key = KEYS.get(entity);
        if (key != null) {
            return key;
        }
        if (entity instanceof Player player) {
            key = "player:" + player.getScoreboardName();
        } else {
            String type = entity.typeHolder().getRegisteredName();
            String base = type.substring(type.indexOf(':') + 1)
                + "@" + entity.getBlockX() + "," + entity.getBlockY() + "," + entity.getBlockZ();
            int n = REGISTRATIONS.merge(base, 1, Integer::sum) - 1;
            key = base + "#" + n;
        }
        KEYS.put(entity, key);
        return key;
    }
}
