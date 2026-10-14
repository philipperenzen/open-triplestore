<script lang="ts">
  // ShEx validation over everything the caller may read (POST /api/shex/validate).
  import { t } from 'svelte-i18n';
  import { ShieldCheck } from 'lucide-svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ShexValidator from '../components/reasoning/ShexValidator.svelte';
</script>

<PageHeader title={$t('pages.shex.title')} icon={ShieldCheck} />
<ShexValidator />
