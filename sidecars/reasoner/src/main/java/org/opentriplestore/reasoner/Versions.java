package org.opentriplestore.reasoner;

import org.semanticweb.HermiT.Reasoner;
import org.semanticweb.owlapi.util.VersionInfo;

final class Versions {
    private Versions() {}

    static String hermit() {
        Package p = Reasoner.class.getPackage();
        String v = p == null ? null : p.getImplementationVersion();
        return v != null ? trim(v) : "1.4.5.519";
    }

    /** "1.4.5.519.2020-02-18T20:48:14Z" → "1.4.5.519" (drop the build stamp). */
    static String trim(String v) {
        return v.replaceFirst("\\.\\d{4}-\\d{2}-\\d{2}T.*$", "");
    }

    static String owlapi() {
        return trim(VersionInfo.getVersionInfo().getVersion());
    }

    static String sidecar() {
        Package p = Versions.class.getPackage();
        String v = p == null ? null : p.getImplementationVersion();
        return v != null ? v : "dev";
    }
}
