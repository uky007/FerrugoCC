// ExportDecomp.java — Ghidra headless script to export decompilation results
// Usage: analyzeHeadless ... -postScript ExportDecomp.java <output_dir>

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import java.io.*;

public class ExportDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        String outDir = args.length > 0 ? args[0] : "/tmp/ghidra_out";
        new File(outDir).mkdirs();

        DecompInterface decomp = new DecompInterface();
        decomp.openProgram(currentProgram);

        PrintWriter funcWriter = new PrintWriter(new File(outDir, "functions.txt"));
        PrintWriter cWriter = new PrintWriter(new File(outDir, "decompiled.c"));
        PrintWriter metaWriter = new PrintWriter(new File(outDir, "meta.txt"));

        FunctionIterator funcs = currentProgram.getListing().getFunctions(true);
        int funcCount = 0;
        int decompOk = 0;
        int namedCount = 0;
        int totalLines = 0;
        int gotoCount = 0;

        while (funcs.hasNext()) {
            Function f = funcs.next();
            funcCount++;
            String name = f.getName();
            funcWriter.println(name);

            // Count "real" named functions (not FUN_ or thunks)
            if (!name.startsWith("FUN_") && !name.startsWith("_") && !name.equals("entry")) {
                namedCount++;
            }

            DecompileResults res = decomp.decompileFunction(f, 30, monitor);
            if (res.decompileCompleted()) {
                decompOk++;
                String c = res.getDecompiledFunction().getC();
                cWriter.println("// === " + name + " ===");
                cWriter.println(c);

                // Count lines and gotos
                String[] lines = c.split("\n");
                totalLines += lines.length;
                for (String line : lines) {
                    if (line.contains("goto ")) {
                        gotoCount++;
                    }
                }
            }
        }

        metaWriter.println("functions=" + funcCount);
        metaWriter.println("decompiled=" + decompOk);
        metaWriter.println("named=" + namedCount);
        metaWriter.println("lines=" + totalLines);
        metaWriter.println("gotos=" + gotoCount);

        funcWriter.close();
        cWriter.close();
        metaWriter.close();
        decomp.dispose();

        println("ExportDecomp: " + funcCount + " functions, " + decompOk + " decompiled, " +
                namedCount + " named, " + totalLines + " lines, " + gotoCount + " gotos");
    }
}
