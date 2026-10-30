package parity.rng;

import java.lang.instrument.Instrumentation;

/**
 * Java agent for the parity RNG lockdown. With {@code -Dparity.rng.seed=<long>} it routes
 * {@code Entity.random} and {@code Level.random} through per-owner streams (see {@link Router});
 * without the property it does nothing.
 *
 * <p>This class must not touch Minecraft classes: they have to load after the transformer is
 * registered.
 */
public final class Agent {
    private Agent() {}

    public static void premain(String args, Instrumentation inst) {
        if (System.getProperty("parity.rng.seed") == null) {
            return;
        }
        inst.addTransformer(new Transformer());
    }
}
