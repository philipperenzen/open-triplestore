#!/usr/bin/env bash
# Generate the deterministic comparison dataset: N persons × 5 properties as
# N-Triples. Matches the criterion `gen_persons` workload so the in-process and
# cross-store numbers describe the same data. Default 100k persons = 500k triples.
#   ./gen-data.sh [N] > data.nt
set -euo pipefail
N="${1:-100000}"
awk -v N="$N" 'BEGIN{
  xsd="http://www.w3.org/2001/XMLSchema#"; ex="http://example.org/";
  for(i=0;i<N;i++){
    age=18+(i%65); kind=i%10; s=(i*7.13); s=s-int(s/100)*100;
    p="<" ex "p" i ">";
    printf "%s <%sname> \"Person %d\" .\n", p, ex, i;
    printf "%s <%sage> \"%d\"^^<%sinteger> .\n", p, ex, age, xsd;
    printf "%s <%stype> <%sType%d> .\n", p, ex, ex, kind;
    printf "%s <%sscore> \"%.2f\"^^<%sdecimal> .\n", p, ex, s, xsd;
    printf "%s <%semail> \"person%d@example.org\" .\n", p, ex, i;
  }
}'
