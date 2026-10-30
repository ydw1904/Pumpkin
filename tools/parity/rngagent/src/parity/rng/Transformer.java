package parity.rng;

import java.lang.classfile.ClassFile;
import java.lang.classfile.ClassHierarchyResolver;
import java.lang.classfile.ClassModel;
import java.lang.classfile.ClassTransform;
import java.lang.classfile.CodeBuilder;
import java.lang.classfile.CodeElement;
import java.lang.classfile.CodeTransform;
import java.lang.classfile.MethodModel;
import java.lang.classfile.MethodTransform;
import java.lang.classfile.Opcode;
import java.lang.classfile.instruction.InvokeInstruction;
import java.lang.classfile.instruction.ReturnInstruction;
import java.lang.constant.ClassDesc;
import java.lang.constant.ConstantDescs;
import java.lang.constant.MethodTypeDesc;
import java.lang.instrument.ClassFileTransformer;
import java.security.ProtectionDomain;

/**
 * Rewrites five Minecraft classes:
 * <ul>
 *   <li>{@code Entity} and {@code Level}: {@code RandomSource.create()} in the constructors (the
 *       {@code random} field) becomes {@link Router#routedRandom()}.</li>
 *   <li>{@code ServerLevel}: entity, passenger, block, fluid and chunk ticks push their owner on
 *       entry and pop it before every return; {@code addEntity} and {@code addPlayer} register the
 *       entity's owner key first.</li>
 *   <li>{@code ServerChunkCache}: {@code tickChunks} runs as {@code chunks} and each
 *       {@code tickSpawningChunk} as that chunk.</li>
 *   <li>{@code ServerTickRateManager.setFrozen}: calls {@link Router#setFrozen} first.</li>
 * </ul>
 */
final class Transformer implements ClassFileTransformer {
    private static final ClassDesc ROUTER = ClassDesc.of("parity.rng.Router");
    private static final ClassDesc RANDOM_SOURCE = ClassDesc.of("net.minecraft.util.RandomSource");
    private static final ClassDesc ENTITY = ClassDesc.of("net.minecraft.world.entity.Entity");
    private static final ClassDesc BLOCK_POS = ClassDesc.of("net.minecraft.core.BlockPos");
    private static final ClassDesc LEVEL_CHUNK = ClassDesc.of("net.minecraft.world.level.chunk.LevelChunk");
    private static final MethodTypeDesc VOID = MethodTypeDesc.of(ConstantDescs.CD_void);

    @Override
    public byte[] transform(ClassLoader loader, String name, Class<?> redefined, ProtectionDomain domain, byte[] bytes) {
        try {
            return switch (name) {
                case "net/minecraft/world/entity/Entity", "net/minecraft/world/level/Level" ->
                    rewrite(loader, bytes, Transformer::routeRandomField);
                case "net/minecraft/server/level/ServerLevel" -> rewrite(loader, bytes, Transformer::serverLevel);
                case "net/minecraft/server/ServerTickRateManager" -> rewrite(loader, bytes, Transformer::tickRateManager);
                case "net/minecraft/server/level/ServerChunkCache" -> rewrite(loader, bytes, Transformer::chunkCache);
                default -> null;
            };
        } catch (Throwable t) {
            // The JVM swallows transformer exceptions; a silent failure would make the run look unrouted.
            System.err.println("[parity.rng] failed to transform " + name);
            t.printStackTrace();
            return null;
        }
    }

    private interface Picker {
        CodeTransform pick(MethodModel method);
    }

    private static byte[] rewrite(ClassLoader loader, byte[] bytes, Picker picker) {
        ClassHierarchyResolver resolver = loader == null
            ? ClassHierarchyResolver.defaultResolver()
            : ClassHierarchyResolver.ofResourceParsing(loader).orElse(ClassHierarchyResolver.defaultResolver());
        ClassFile cf = ClassFile.of(ClassFile.ClassHierarchyResolverOption.of(resolver));
        ClassModel model = cf.parse(bytes);
        ClassTransform transform = (builder, element) -> {
            CodeTransform code = element instanceof MethodModel method ? picker.pick(method) : null;
            if (code == null) {
                builder.with(element);
            } else {
                builder.transformMethod((MethodModel) element, MethodTransform.transformingCode(code));
            }
        };
        return cf.transformClass(model, transform);
    }

    private static CodeTransform routeRandomField(MethodModel method) {
        if (!method.methodName().equalsString("<init>")) {
            return null;
        }
        return (builder, element) -> {
            if (element instanceof InvokeInstruction call
                && call.opcode() == Opcode.INVOKESTATIC
                && call.owner().asInternalName().equals("net/minecraft/util/RandomSource")
                && call.name().equalsString("create")
                && call.type().equalsString("()Lnet/minecraft/util/RandomSource;")) {
                builder.invokestatic(ROUTER, "routedRandom", MethodTypeDesc.of(RANDOM_SOURCE));
            } else {
                builder.with(element);
            }
        };
    }

    private static CodeTransform serverLevel(MethodModel method) {
        String key = method.methodName().stringValue() + method.methodType().stringValue();
        return switch (key) {
            case "tickNonPassenger(Lnet/minecraft/world/entity/Entity;)V" -> scoped(1, "enterEntity", ENTITY);
            // tickPassenger(vehicle, passenger): the passenger is the one ticking.
            case "tickPassenger(Lnet/minecraft/world/entity/Entity;Lnet/minecraft/world/entity/Entity;)V" ->
                scoped(2, "enterEntity", ENTITY);
            case "tickBlock(Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/level/block/Block;)V" ->
                scoped(1, "enterBlock", BLOCK_POS);
            case "tickFluid(Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/level/material/Fluid;)V" ->
                scoped(1, "enterFluid", BLOCK_POS);
            case "tickChunk(Lnet/minecraft/world/level/chunk/LevelChunk;I)V" -> scoped(1, "enterChunk", LEVEL_CHUNK);
            case "addEntity(Lnet/minecraft/world/entity/Entity;)Z",
                 "addPlayer(Lnet/minecraft/server/level/ServerPlayer;)V" -> registering();
            default -> null;
        };
    }

    private static CodeTransform chunkCache(MethodModel method) {
        String key = method.methodName().stringValue() + method.methodType().stringValue();
        return switch (key) {
            // The spawning-chunk shuffle and custom spawners; per-chunk work pushes its own owner.
            case "tickChunks(Lnet/minecraft/util/profiling/ProfilerFiller;)V" -> scoped(-1, "enterChunks", null);
            // Lightning and natural spawning for one chunk.
            case "tickSpawningChunk(Lnet/minecraft/world/level/chunk/LevelChunk;Ljava/util/List;Lnet/minecraft/world/level/NaturalSpawner$SpawnState;)V" ->
                scoped(1, "enterChunk", LEVEL_CHUNK);
            default -> null;
        };
    }

    private static CodeTransform tickRateManager(MethodModel method) {
        if (!(method.methodName().equalsString("setFrozen") && method.methodType().equalsString("(Z)V"))) {
            return null;
        }
        return new CodeTransform() {
            @Override
            public void atStart(CodeBuilder builder) {
                builder.iload(1);
                builder.invokestatic(ROUTER, "setFrozen", MethodTypeDesc.of(ConstantDescs.CD_void, ConstantDescs.CD_boolean));
            }

            @Override
            public void accept(CodeBuilder builder, CodeElement element) {
                builder.with(element);
            }
        };
    }

    /**
     * Pushes the owner taken from argument {@code slot} on entry and pops it before each return.
     * With {@code slot} -1, {@code enter} takes no argument.
     */
    private static CodeTransform scoped(int slot, String enter, ClassDesc argType) {
        return new CodeTransform() {
            @Override
            public void atStart(CodeBuilder builder) {
                if (slot < 0) {
                    builder.invokestatic(ROUTER, enter, VOID);
                } else {
                    builder.aload(slot);
                    builder.invokestatic(ROUTER, enter, MethodTypeDesc.of(ConstantDescs.CD_void, argType));
                }
            }

            @Override
            public void accept(CodeBuilder builder, CodeElement element) {
                if (element instanceof ReturnInstruction) {
                    builder.invokestatic(ROUTER, "exit", VOID);
                }
                builder.with(element);
            }
        };
    }

    private static CodeTransform registering() {
        return new CodeTransform() {
            @Override
            public void atStart(CodeBuilder builder) {
                builder.aload(1);
                builder.invokestatic(ROUTER, "register", MethodTypeDesc.of(ConstantDescs.CD_void, ENTITY));
            }

            @Override
            public void accept(CodeBuilder builder, CodeElement element) {
                builder.with(element);
            }
        };
    }
}
