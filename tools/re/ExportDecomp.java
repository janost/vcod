// ExportDecomp.java: Ghidra script that decompiles every function of the current
// program into one <binary>.c file in the working directory, each function under a
// "// --- name @ addr" marker so the dump can be grepped by address. Java, not
// Python, because not every Ghidra build ships Jython/PyGhidra.
//
// Import and analyze the binary once, then run this without re-analysis:
//   analyzeHeadless <projdir> <proj> -import <binary>
//   analyzeHeadless <projdir> <proj> -process <binary> -noanalysis -scriptPath tools/re -postScript ExportDecomp.java
//@category Analysis
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.listing.Function;
import java.io.PrintWriter;
import java.io.File;

public class ExportDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        String outPath = System.getProperty("user.dir") + File.separator
            + currentProgram.getName() + ".c";
        PrintWriter out = new PrintWriter(new File(outPath));
        DecompInterface ifc = new DecompInterface();
        ifc.openProgram(currentProgram);
        int n = 0;
        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            DecompileResults r = ifc.decompileFunction(f, 120, monitor);
            if (r.decompileCompleted()) {
                out.print("// --- " + f.getName() + " @ " + f.getEntryPoint() + "\n");
                out.print(r.getDecompiledFunction().getC());
                out.print("\n");
                n++;
            }
        }
        out.close();
        println("wrote " + outPath + " (" + n + " functions)");
    }
}
