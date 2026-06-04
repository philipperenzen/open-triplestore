<script>
  // The standalone /graph-viz page was merged into /browse?view=graph so the
  // table and graph views share filters, pagination state, and the same
  // backend calls. This component preserves deep-links to the old URL by
  // forwarding every query parameter (uri, graph, dataset, org, subject,
  // predicate, object, q) to the browse page.
  import { onMount } from 'svelte';
  import { navigate } from '../lib/router/index.js';

  onMount(() => {
    const incoming = new URLSearchParams(window.location.search);
    const out = new URLSearchParams();
    out.set('view', 'graph');
    for (const key of ['graph', 'dataset', 'org', 'subject', 'predicate', 'object', 'q', 'uri']) {
      const v = incoming.get(key);
      if (v) out.set(key, v);
    }
    navigate(`/browse?${out.toString()}`, { replace: true });
  });
</script>

<p style="padding: 2rem; color: #64748b;">Redirecting to the unified browse view…</p>
