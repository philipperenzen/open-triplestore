<!-- Stands in for a heavy lazily-imported child (GraphCanvas, ViewerMap) and
     reports what it was handed: the length of every array prop, and the ids
     of `elements`/`nodes`, so a test can assert what would have been drawn. -->
<script>
  $: summary = Object.fromEntries(
    Object.entries($$props)
      .filter(([, v]) => Array.isArray(v))
      .map(([k, v]) => [k, v.length]),
  );
  $: ids = [...($$props.elements || []), ...($$props.nodes || []).map((n) => n.data)].map((e) => e.id);
</script>

<div data-testid="probe" data-summary={JSON.stringify(summary)} data-ids={JSON.stringify(ids)}></div>
