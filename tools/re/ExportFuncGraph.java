// ExportFuncGraph.java: dumps every function's matching features to
// <program>.funcs.tsv in the working directory, one function per line:
//   entry \t name \t size \t calls \t strings \t data refs \t scalars \t tokens
// calls / data refs are comma-separated hex addresses in instruction order,
// strings are \x1f-separated, scalars the distinct immediates >= 0x100;
// tokens are all of them in instruction order, \x1e-separated, each
// "c:<addr>" (call or function pointer), "s:<string>" or "d:<addr>".
// Run after analysis:
//   analyzeHeadless <projdir> <proj> -process <bin> -noanalysis -scriptPath <dir> -postScript ExportFuncGraph.java
//@category Analysis
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.Reference;
import java.io.File;
import java.io.PrintWriter;
import java.util.*;

public class ExportFuncGraph extends GhidraScript {
    String cstr(Address a) {
        MemoryBlock b = currentProgram.getMemory().getBlock(a);
        if (b == null || b.isExecute()) return null;
        StringBuilder sb = new StringBuilder();
        try {
            for (int i = 0; i < 256; i++) {
                byte c = currentProgram.getMemory().getByte(a.add(i));
                if (c == 0) break;
                if (c < 0x09 || (c > 0x0d && c < 0x20) || c > 0x7e) return null;
                sb.append((char) c);
            }
        } catch (Exception e) {
            return null;
        }
        if (sb.length() < 3) return null;
        return sb.toString().replace('\t', ' ').replace('\n', ' ').replace('\r', ' ').replace('\u001f', ' ');
    }

    @Override
    public void run() throws Exception {
        String outPath = System.getProperty("user.dir") + File.separator
            + currentProgram.getName() + ".funcs.tsv";
        PrintWriter out = new PrintWriter(new File(outPath));
        FunctionManager fm = currentProgram.getFunctionManager();
        Listing listing = currentProgram.getListing();
        for (Function f : fm.getFunctions(true)) {
            List<String> calls = new ArrayList<>(), strs = new ArrayList<>(), data = new ArrayList<>(), toks = new ArrayList<>();
            TreeSet<Long> scalars = new TreeSet<>();
            for (Instruction ins : listing.getInstructions(f.getBody(), true)) {
                for (Reference r : ins.getReferencesFrom()) {
                    Address to = r.getToAddress();
                    if (!to.isMemoryAddress()) continue;
                    if (r.getReferenceType().isCall()) {
                        calls.add(to.toString());
                        toks.add("c:" + to);
                        continue;
                    }
                    if (r.getReferenceType().isFlow()) continue;
                    Function g = fm.getFunctionAt(to);
                    if (g != null) { calls.add("&" + to); toks.add("c:" + to); continue; }
                    String s = cstr(to);
                    if (s != null) { strs.add(s); toks.add("s:" + s); } else { data.add(to.toString()); toks.add("d:" + to); }
                }
                for (int i = 0; i < ins.getNumOperands(); i++) {
                    for (Object o : ins.getOpObjects(i)) {
                        if (o instanceof Scalar) {
                            long v = ((Scalar) o).getUnsignedValue();
                            if (v >= 0x100 && v < 0x1000000) scalars.add(v);
                        }
                    }
                }
            }
            List<String> sc = new ArrayList<>();
            for (Long v : scalars) sc.add(Long.toHexString(v));
            out.print(f.getEntryPoint() + "\t" + f.getName() + "\t" + f.getBody().getNumAddresses()
                + "\t" + String.join(",", calls) + "\t" + String.join("\u001f", strs)
                + "\t" + String.join(",", data) + "\t" + String.join(",", sc)
                + "\t" + String.join("\u001e", toks) + "\n");
        }
        out.close();
        println("wrote " + outPath);
    }
}
