package parity.rng;

import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;

/**
 * A RandomSource whose primitive draws come from the current owner's stream, not from this object.
 * The interface's default methods (triangle, nextIntBetweenInclusive, nextInt(a, b), ...) are left
 * alone so they decompose into logged primitives.
 */
final class RoutedRandom implements RandomSource {
    /** Set by {@link #setSeed}: from then on this instance is an ordinary, unrouted LCG. */
    private LegacyRandomSource own;

    @Override
    public RandomSource fork() {
        return own != null ? own.fork() : new LegacyRandomSource(nextLong());
    }

    @Override
    public PositionalRandomFactory forkPositional() {
        // Vanilla LegacyRandomSource.forkPositional.
        return own != null ? own.forkPositional() : new LegacyRandomSource.LegacyPositionalRandomFactory(nextLong());
    }

    @Override
    public void setSeed(long seed) {
        own = new LegacyRandomSource(seed);
    }

    @Override
    public int nextInt() {
        if (own != null) return own.nextInt();
        String owner = Router.owner();
        LegacyRandomSource s = Router.stream(owner);
        int r;
        synchronized (s) {
            r = s.nextInt();
        }
        DrawLog.write(owner, "nextInt", "-", Integer.toString(r));
        return r;
    }

    @Override
    public int nextInt(int bound) {
        if (own != null) return own.nextInt(bound);
        String owner = Router.owner();
        LegacyRandomSource s = Router.stream(owner);
        int r;
        synchronized (s) {
            r = s.nextInt(bound);
        }
        DrawLog.write(owner, "nextIntBounded", Integer.toString(bound), Integer.toString(r));
        return r;
    }

    @Override
    public long nextLong() {
        if (own != null) return own.nextLong();
        String owner = Router.owner();
        LegacyRandomSource s = Router.stream(owner);
        long r;
        synchronized (s) {
            r = s.nextLong();
        }
        DrawLog.write(owner, "nextLong", "-", Long.toString(r));
        return r;
    }

    @Override
    public boolean nextBoolean() {
        if (own != null) return own.nextBoolean();
        String owner = Router.owner();
        LegacyRandomSource s = Router.stream(owner);
        boolean r;
        synchronized (s) {
            r = s.nextBoolean();
        }
        DrawLog.write(owner, "nextBoolean", "-", Boolean.toString(r));
        return r;
    }

    @Override
    public float nextFloat() {
        if (own != null) return own.nextFloat();
        String owner = Router.owner();
        LegacyRandomSource s = Router.stream(owner);
        float r;
        synchronized (s) {
            r = s.nextFloat();
        }
        DrawLog.write(owner, "nextFloat", "-", DrawLog.hex(r));
        return r;
    }

    @Override
    public double nextDouble() {
        if (own != null) return own.nextDouble();
        String owner = Router.owner();
        LegacyRandomSource s = Router.stream(owner);
        double r;
        synchronized (s) {
            r = s.nextDouble();
        }
        DrawLog.write(owner, "nextDouble", "-", DrawLog.hex(r));
        return r;
    }

    @Override
    public double nextGaussian() {
        if (own != null) return own.nextGaussian();
        String owner = Router.owner();
        LegacyRandomSource s = Router.stream(owner);
        double r;
        synchronized (s) {
            r = s.nextGaussian();
        }
        DrawLog.write(owner, "nextGaussian", "-", DrawLog.hex(r));
        return r;
    }
}
