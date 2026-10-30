// ApplySyms.java: applies a symmap.py / merge_zk.py names file to the current
// program. "T" lines name (or create) the function at the address, "D" lines
// put a primary label there. Characters Ghidra rejects become '_'.
//   analyzeHeadless <projdir> <proj> -process <bin> -noanalysis -scriptPath <dir> -postScript ApplySyms.java <syms>
//@category Analysis
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.SourceType;
import java.io.BufferedReader;
import java.io.FileReader;

public class ApplySyms extends GhidraScript {
    @Override
    public void run() throws Exception {
        String path = getScriptArgs()[0];
        int funcs = 0, created = 0, labels = 0, failed = 0;
        try (BufferedReader r = new BufferedReader(new FileReader(path))) {
            String line;
            while ((line = r.readLine()) != null) {
                if (line.startsWith("#") || line.isBlank()) continue;
                String[] f = line.trim().split("\\s+");
                Address a = toAddr(Long.parseLong(f[0], 16));
                String name = f[2].replaceAll("[^A-Za-z0-9_~.$]", "_");
                try {
                    if (f[1].equals("T")) {
                        Function fn = getFunctionAt(a);
                        if (fn == null) {
                            fn = createFunction(a, name);
                            if (fn == null) { failed++; continue; }
                            created++;
                        }
                        fn.setName(name, SourceType.USER_DEFINED);
                        funcs++;
                    } else {
                        createLabel(a, name, true, SourceType.USER_DEFINED);
                        labels++;
                    }
                } catch (Exception e) {
                    println("failed " + line + ": " + e.getMessage());
                    failed++;
                }
            }
        }
        println("ApplySyms: functions " + funcs + " (created " + created + "), labels " + labels + ", failed " + failed);
    }
}
