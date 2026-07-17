<script>
  import { Router, Route, Link, navigate } from './lib/router/index.js';
  import { onMount, tick } from 'svelte';
  import { t, locale, isLoading } from 'svelte-i18n';
  import { isAuthenticated, user, isAdmin, refreshUser, backendHealth, checkBackend } from './lib/stores.js';
  import { logout as apiLogout } from './lib/api.js';
  import { location } from './lib/locationStore.js';
  import Toasts from './components/Toasts.svelte';
  import SearchBar from './components/SearchBar.svelte';
  import LoadingLogo from './components/LoadingLogo.svelte';

  import {
    Home as HomeIcon, Search as SearchIcon,
    Upload, Database, Building2, BookOpen, HelpCircle,
    LogIn, LogOut, UserPlus, Menu, X, Globe, AlertTriangle, RefreshCw,
    Settings as SettingsIcon, Users as UsersIcon, Shield,
    Share2, Terminal, CheckCircle2, Network, FileCode, Sparkles, Sun, Moon, Activity
  } from 'lucide-svelte';
  import { isDark, toggleTheme } from './lib/theme.js';
  import { runtimeBranding } from './lib/runtimeConfig.js';

  import Home from './pages/Home.svelte';
  import Login from './pages/Login.svelte';
  import Register from './pages/Register.svelte';
  import ForgotPassword from './pages/ForgotPassword.svelte';
  import ResetPassword from './pages/ResetPassword.svelte';
  import VerifyEmail from './pages/VerifyEmail.svelte';
  import OAuthCallback from './pages/OAuthCallback.svelte';
  import Settings from './pages/Settings.svelte';
  import Datasets from './pages/Datasets.svelte';
  import DatasetDetail from './pages/DatasetDetail.svelte';
  import Organisations from './pages/Organisations.svelte';
  import OrgDetail from './pages/OrgDetail.svelte';
  import GraphList from './pages/GraphList.svelte';
  import ResourceDetail from './pages/ResourceDetail.svelte';
  import PreviewOverlay from './components/viewer/PreviewOverlay.svelte';
  import Validation from './pages/Validation.svelte';

  // W4-20: Heavy pages use dynamic imports so their vendor chunks (CodeMirror,
  // Cytoscape, etc.) are only fetched when the route is first visited.
  import LazyPage from './components/LazyPage.svelte';
  const lazySparqlEditor     = () => import('./pages/SparqlEditor.svelte');
  const lazyDatasetViewer    = () => import('./pages/DatasetViewer.svelte');
  const lazyCesiumView       = () => import('./pages/CesiumView.svelte');
  const lazyApiServices      = () => import('./pages/ApiServices.svelte');
  // /graph-viz is deprecated: the unified browse page (/browse?view=graph) now
  // owns the graph viz. Keep a thin redirect so deep-links don't 404.
  const lazyGraphVisualizer  = () => import('./pages/GraphVizRedirect.svelte');
  const lazyDataImport          = () => import('./pages/DataImport.svelte');
  const lazyShaclStudio         = () => import('./pages/ShaclStudio.svelte');
  const lazyShapeLibrary        = () => import('./pages/ShapeLibrary.svelte');
  const lazyShapeGraphEditor      = () => import('./pages/ShapeGraphEditor.svelte');
  const lazyPipelinesList       = () => import('./pages/PipelinesList.svelte');
  const lazyPipelineEditor      = () => import('./pages/PipelineEditor.svelte');
  const lazyShaclResults        = () => import('./pages/ShaclResults.svelte');
  const lazyAdminUsers          = () => import('./pages/AdminUsers.svelte');
  const lazyAdminSecurity       = () => import('./pages/AdminSecurity.svelte');
  const lazyAdminLlm            = () => import('./pages/AdminLlm.svelte');
  const lazyDocEditor           = () => import('./pages/DocEditor.svelte');
  const lazyModelRegistry       = () => import('./pages/ModelRegistry.svelte');
  const lazyModelDetail         = () => import('./pages/ModelDetail.svelte');
  const lazyModelViewer         = () => import('./pages/ModelViewer.svelte');
  const lazyModelDiff           = () => import('./pages/ModelDiff.svelte');
  const lazyTripleBrowser       = () => import('./pages/TripleBrowser.svelte');
  const lazyLlmChat             = () => import('./pages/LlmChat.svelte');
  const lazyDocumentation    = () => import('./pages/Documentation.svelte');
  const lazyApiDocs          = () => import('./pages/ApiDocs.svelte');

  // Overridable at runtime (no rebuild) via /config.json's "branding.title" — see runtimeConfig.ts.
  $: BRAND = $runtimeBranding.title;

  const NAV_SECTIONS = [
    {
      titleKey: 'nav.workspace',
      items: [
        { to: '/', labelKey: 'nav.overview', icon: HomeIcon, match: (p) => p === '/' },
      ],
    },
    {
      titleKey: 'nav.explore',
      items: [
        { to: '/browse', labelKey: 'nav.exploreTriples', icon: Share2, match: (p) => p.startsWith('/browse') || p.startsWith('/resource') },
        { to: '/sparql', labelKey: 'nav.sparqlWorkspace', icon: Terminal, match: (p) => p === '/sparql' },
        { to: '/chat', labelKey: 'nav.llmChat', icon: Sparkles, match: (p) => p === '/chat' },
      ],
    },
    {
      titleKey: 'nav.operations',
      items: [
        { to: '/import', labelKey: 'nav.importData', icon: Upload, match: (p) => p.startsWith('/import'), authRequired: true },
        { to: '/shacl', labelKey: 'nav.validate', icon: CheckCircle2, match: (p) => p.startsWith('/validation') || p.startsWith('/shacl'), authRequired: true },
      ],
    },
    {
      titleKey: 'nav.manage',
      items: [
        { to: '/graphs', labelKey: 'nav.namedGraphs', icon: Network, match: (p) => p === '/graphs' },
        { to: '/datasets', labelKey: 'nav.datasets', icon: Database, match: (p) => p.startsWith('/datasets') },
        { to: '/organisations', labelKey: 'nav.organisations', icon: Building2, match: (p) => p.startsWith('/organisations') },
        { to: '/models', labelKey: 'nav.models', icon: BookOpen, match: (p) => p.startsWith('/models') },
      ],
    },
    {
      titleKey: 'nav.reference',
      items: [
        { to: '/docs', labelKey: 'nav.documentation', icon: HelpCircle, match: (p) => p.startsWith('/docs') },
        { to: '/api-docs', labelKey: 'nav.apiReference', icon: FileCode, match: (p) => p.startsWith('/api-docs') },
      ],
    },
  ];

  const BOTTOM_TABS = [
    { to: '/', labelKey: 'nav.overview', icon: HomeIcon, match: (p) => p === '/' },
    { to: '/browse', labelKey: 'nav.exploreTriples', icon: Share2, match: (p) => p.startsWith('/browse') },
    { to: '/sparql', labelKey: 'nav.sparqlWorkspace', icon: Terminal, match: (p) => p === '/sparql' },
    { to: '/datasets', labelKey: 'nav.datasets', icon: Database, match: (p) => p.startsWith('/datasets') },
  ];

  // Pages where ⌘K search is meaningful. Auth/settings/admin pages don't need it.
  const SEARCH_ENABLED_PREFIXES = ['/', '/browse', '/resource', '/sparql', '/graph-viz', '/graphs', '/datasets', '/organisations', '/models', '/import', '/validation', '/shacl'];
  function searchEnabledFor(path) {
    if (path === '/') return true;
    return SEARCH_ENABLED_PREFIXES.some((p) => p !== '/' && path.startsWith(p));
  }

  let authed = false;
  let currentUser = null;
  let sidebarOpen = false;
  let searchOpen = false;
  let langMenuOpen = false;
  let healthPopoverOpen = false;
  let healthRefreshing = false;
  let searchBarRef;
  let healthBtnRef;
  let healthPopoverEl;
  let healthPopoverStyle = '';

  async function refreshHealth() {
    if (healthRefreshing) return;
    healthRefreshing = true;
    await checkBackend();
    healthRefreshing = false;
  }

  isAuthenticated.subscribe((value) => (authed = value));
  user.subscribe((value) => (currentUser = value));

  // Close sidebar on any route change (catches all navigation, not just nav-link clicks)
  location.subscribe(() => { sidebarOpen = false; });

  onMount(() => {
    refreshUser();
    checkBackend();
    const pollInterval = setInterval(checkBackend, 30_000);

    function handleKeydown(e) {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        if (!searchEnabledFor(window.location.pathname)) return;
        e.preventDefault();
        searchOpen = !searchOpen;
        if (searchOpen) setTimeout(() => searchBarRef?.focus(), 50);
      }
      if (e.key === 'Escape') {
        sidebarOpen = false;
        searchOpen = false;
        langMenuOpen = false;
        healthPopoverOpen = false;
      }
    }
    window.addEventListener('keydown', handleKeydown);

    function handleAuthExpired() {
      refreshUser();
      navigate('/login');
    }
    window.addEventListener('auth-expired', handleAuthExpired);

    function handleClickOutside(e) {
      if (!healthPopoverOpen) return;
      const inWidget = healthBtnRef?.closest('.health-widget').contains(e.target);
      const inPopover = healthPopoverEl?.contains(e.target);
      if (!inWidget && !inPopover) healthPopoverOpen = false;
    }
    window.addEventListener('mousedown', handleClickOutside);

    // Position the sliding nav selector once mounted, and re-measure after web
    // fonts load (reflow) and on window resize.
    updateNavIndicator();
    document.fonts?.ready.then(measureNavIndicator).catch(() => {});
    window.addEventListener('resize', measureNavIndicator);

    return () => {
      clearInterval(pollInterval);
      window.removeEventListener('keydown', handleKeydown);
      window.removeEventListener('auth-expired', handleAuthExpired);
      window.removeEventListener('mousedown', handleClickOutside);
      window.removeEventListener('resize', measureNavIndicator);
    };
  });

  async function logout() {
    try { await apiLogout(); } catch { /* best effort */ }
    refreshUser();
    navigate('/');
    sidebarOpen = false;
  }

  function navClick() { sidebarOpen = false; }

  function setLocale(loc) {
    locale.set(loc);
    langMenuOpen = false;
  }

  function getPageMeta(pathname) {
    const map = [
      ['/', 'pages.home.title', 'pages.home.detail'],
      ['/browse', 'pages.tripleBrowser.title', 'pages.tripleBrowser.detail'],
      ['/resource', 'pages.resource.title', 'pages.resource.detail'],
      ['/graph-viz', 'pages.graphViz.title', 'pages.graphViz.detail'],
      ['/sparql', 'pages.sparql.title', 'pages.sparql.detail'],
      ['/chat', 'pages.llmChat.title', 'pages.llmChat.detail'],
      ['/import', 'pages.import.title', 'pages.import.detail'],
      ['/validation', 'pages.validation.title', 'pages.validation.detail'],
      ['/shacl', 'pages.shaclStudio.title', 'pages.shaclStudio.detail'],
      ['/graphs', 'pages.graphList.title', 'pages.graphList.detail'],
      ['/datasets', 'pages.datasets.title', 'pages.datasets.detail'],
      ['/organisations', 'pages.organisations.title', 'pages.organisations.detail'],
      ['/login', 'pages.login.title', 'pages.login.detail'],
      ['/register', 'pages.register.title', 'pages.register.detail'],
      ['/settings', 'pages.settings.title', 'pages.settings.detail'],
      ['/admin/llm', 'pages.adminLlm.title', 'pages.adminLlm.detail'],
      ['/admin', 'pages.admin.title', 'pages.admin.detail'],
      ['/models', 'pages.modelRegistry.title', 'pages.modelRegistry.detail'],
      ['/docs', 'pages.documentation.title', 'pages.documentation.detail'],
      ['/api-docs', 'pages.apiDocs.title', 'pages.apiDocs.detail'],
    ];
    for (const [prefix, tk, dk] of map) {
      if (prefix === '/' ? pathname === '/' : pathname.startsWith(prefix)) {
        return {
          title: tk ? $t(tk) : '',
          detail: dk ? $t(dk) : '',
        };
      }
    }
    return { title: $t('nav.workspace'), detail: '' };
  }

  $: currentPath = $location.pathname;

  // Sliding selector: a single highlight pill that animates from the previous
  // nav item to the newly selected one whenever the route changes.
  let scrollEl;
  let navIndicator = { top: 0, left: 0, width: 0, height: 0, visible: false };
  function measureNavIndicator() {
    if (!scrollEl) return;
    const sel = scrollEl.querySelector('.nav-item.selected');
    if (!sel) { navIndicator = { ...navIndicator, visible: false }; return; }
    navIndicator = {
      top: sel.offsetTop, left: sel.offsetLeft,
      width: sel.offsetWidth, height: sel.offsetHeight, visible: true,
    };
  }
  async function updateNavIndicator() {
    await tick();
    // Measure once the DOM/class change has flushed, then again on the next
    // frame to catch any post-layout settling.
    measureNavIndicator();
    requestAnimationFrame(measureNavIndicator);
  }
  // Re-measure whenever the route, auth/admin state, or locale changes the list.
  $: currentPath, authed, $isAdmin, $locale, updateNavIndicator();
  // $locale is referenced so the header re-translates when the language changes
  $: pageMeta = ($locale, $isLoading) ? { title: '', detail: '' } : getPageMeta(currentPath);
  $: searchEnabled = searchEnabledFor(currentPath);
  $: backendStatusLabel = $backendHealth === null
    ? $t('system.checkingBackend')
    : $backendHealth.status === null
      ? $t('system.backendOffline')
      : $backendHealth.status === 'ok'
        ? $t('system.backendOnline')
        : $t('system.backendDegraded') || 'Backend degraded';
</script>

<svelte:head><title>{BRAND}</title></svelte:head>

{#if $isLoading}
  <div class="app-loading">
    <LoadingLogo size={104} />
  </div>
{:else}
<Router>
  <div class="app-shell">
    <!-- Shared gradient defs for the brand mark (ring + knowledge-graph nodes) -->
    <svg width="0" height="0" aria-hidden="true" focusable="false" style="position:absolute">
      <defs>
        <linearGradient id="otRing" gradientUnits="userSpaceOnUse" x1="13" y1="11" x2="51" y2="55">
          <stop offset="0%" stop-color="#cdf6f1"/>
          <stop offset="48%" stop-color="#7ED6D0"/>
          <stop offset="100%" stop-color="#56b6bd"/>
        </linearGradient>
        <radialGradient id="otNode" cx="0.34" cy="0.28" r="0.85">
          <stop offset="0%" stop-color="#d4f7f2"/>
          <stop offset="45%" stop-color="#6fcdc9"/>
          <stop offset="100%" stop-color="#2F7A8C"/>
        </radialGradient>
      </defs>
    </svg>

    <!-- Mobile top bar -->
    <header class="mobile-topbar lg:hidden">
      <button class="icon-btn" on:click={() => sidebarOpen = !sidebarOpen} aria-label={$t('nav.menu')}>
        {#if sidebarOpen}<X size={22} />{:else}<Menu size={22} />{/if}
      </button>
      <Link to="/" class="brand-link-mobile" aria-label={BRAND}>
        <span class="brand-word brand-word-sm">
          {#if $runtimeBranding.logoUrl}
            <img class="brand-o" src={$runtimeBranding.logoUrl} alt="" aria-hidden="true" /><span class="brand-rest">{BRAND}</span>
          {:else}
            <svg class="brand-o" viewBox="0 0 64 64" fill="none" aria-hidden="true">
              <circle cx="32" cy="32" r="19" stroke="url(#otRing)" stroke-width="4.6"/>
              <g stroke="#dbf7f3" stroke-width="1.9" stroke-opacity="0.5" stroke-linecap="round">
                <line x1="32" y1="51" x2="48.45" y2="22.5"/>
                <line x1="48.45" y1="22.5" x2="15.55" y2="22.5"/>
                <line x1="15.55" y1="22.5" x2="32" y2="51"/>
              </g>
              <g stroke="#eafdfb" stroke-width="2.1">
                <circle cx="32" cy="51" r="6.4" fill="url(#otNode)"/>
                <circle cx="48.45" cy="22.5" r="6.4" fill="url(#otNode)"/>
                <circle cx="15.55" cy="22.5" r="6.4" fill="url(#otNode)"/>
              </g>
            </svg><span class="brand-rest">pen Triplestore</span>
          {/if}
        </span>
      </Link>
      {#if searchEnabled}
        <button class="icon-btn" on:click={() => { searchOpen = !searchOpen; if (searchOpen) setTimeout(() => searchBarRef?.focus(), 50); }} aria-label={$t('system.search')}>
          <SearchIcon size={20} />
        </button>
      {:else}
        <span class="icon-btn-placeholder"></span>
      {/if}
    </header>

    {#if sidebarOpen}
      <button class="sidebar-overlay lg:hidden" on:click={() => sidebarOpen = false} aria-label={$t('nav.closeMenu')}></button>
    {/if}

    <!-- Sidebar -->
    <aside class="sidebar-shell" class:sidebar-open={sidebarOpen}>
      <div class="brand-block">
        <Link to="/" class="brand-link" on:click={navClick} aria-label={BRAND}>
          <span class="brand-word">
            {#if $runtimeBranding.logoUrl}
              <img class="brand-o" src={$runtimeBranding.logoUrl} alt="" aria-hidden="true" /><span class="brand-rest">{BRAND}</span>
            {:else}
              <svg class="brand-o" viewBox="0 0 64 64" fill="none" aria-hidden="true">
                <circle cx="32" cy="32" r="19" stroke="url(#otRing)" stroke-width="4.6"/>
                <g stroke="#dbf7f3" stroke-width="1.9" stroke-opacity="0.5" stroke-linecap="round">
                  <line x1="32" y1="51" x2="48.45" y2="22.5"/>
                  <line x1="48.45" y1="22.5" x2="15.55" y2="22.5"/>
                  <line x1="15.55" y1="22.5" x2="32" y2="51"/>
                </g>
                <g stroke="#eafdfb" stroke-width="2.1">
                  <circle cx="32" cy="51" r="6.4" fill="url(#otNode)"/>
                  <circle cx="48.45" cy="22.5" r="6.4" fill="url(#otNode)"/>
                  <circle cx="15.55" cy="22.5" r="6.4" fill="url(#otNode)"/>
                </g>
              </svg><span class="brand-rest">pen Triplestore</span>
            {/if}
          </span>
          <small class="brand-sub">{$t('nav.brandSub')}</small>
        </Link>
      </div>

      <div class="sidebar-scroll" bind:this={scrollEl}>
        <div
          class="nav-indicator"
          class:visible={navIndicator.visible}
          style="transform: translate({navIndicator.left}px, {navIndicator.top}px); width: {navIndicator.width}px; height: {navIndicator.height}px;"
          aria-hidden="true"
        ></div>
        {#each NAV_SECTIONS as section}
          {#if authed || section.items.some(item => !item.authRequired)}
          <div class="sidebar-section">
            <div class="sidebar-heading">{section.titleKey ? $t(section.titleKey) : section.title}</div>
            <nav class="nav-group" aria-label={section.titleKey ? $t(section.titleKey) : section.title}>
              {#each section.items as item}
                {#if !item.authRequired || authed}
                <Link to={item.to} class={`nav-item ${item.match(currentPath) ? 'selected' : ''}`} on:click={navClick}>
                  <svelte:component this={item.icon} size={16} />
                  <span class="nav-item-label">{item.label || $t(item.labelKey)}</span>
                </Link>
                {/if}
              {/each}
            </nav>
          </div>
          {/if}
        {/each}

        {#if $isAdmin}
          <div class="sidebar-section">
            <div class="sidebar-heading">{$t('nav.admin')}</div>
            <nav class="nav-group" aria-label={$t('nav.admin')}>
              <Link to="/admin/users" class={`nav-item ${currentPath.startsWith('/admin/users') ? 'selected' : ''}`} on:click={navClick}>
                <UsersIcon size={16} />
                <span class="nav-item-label">{$t('nav.users')}</span>
              </Link>
              <Link to="/admin/security" class={`nav-item ${currentPath.startsWith('/admin/security') ? 'selected' : ''}`} on:click={navClick}>
                <Shield size={16} />
                <span class="nav-item-label">{$t('nav.securityAcl')}</span>
              </Link>
              <Link to="/admin/llm" class={`nav-item ${currentPath.startsWith('/admin/llm') ? 'selected' : ''}`} on:click={navClick}>
                <Activity size={16} />
                <span class="nav-item-label">{$t('nav.adminLlm')}</span>
              </Link>
              <Link to="/admin/docs" class={`nav-item ${currentPath.startsWith('/admin/docs') ? 'selected' : ''}`} on:click={navClick}>
                <SettingsIcon size={16} />
                <span class="nav-item-label">{$t('nav.documentation')}</span>
              </Link>
            </nav>
          </div>
        {/if}
      </div>

      <div class="sidebar-footer">
        {#if authed}
          <div class="user-card">
            <div class="user-meta">
              <span class="user-avatar">{currentUser?.username?.slice(0, 1)?.toUpperCase() || 'U'}</span>
              <div class="user-text">
                <strong>{currentUser?.username || $t('nav.signIn')}</strong>
                <small>{currentUser?.role || $t('nav.authenticatedAccess')}</small>
              </div>
            </div>
            <div class="user-actions">
              <Link class="icon-link" to="/settings" on:click={navClick} aria-label={$t('nav.settings')} title={$t('nav.settings')}>
                <SettingsIcon size={14} />
              </Link>
              <button class="icon-link" on:click={logout} aria-label={$t('nav.signOut')} title={$t('nav.signOut')}>
                <LogOut size={14} />
              </button>
            </div>
          </div>
        {:else}
          <div class="auth-actions">
            <Link class="btn btn-sm" to="/login" on:click={navClick}>
              <LogIn size={14} />
              {$t('nav.signIn')}
            </Link>
            <Link class="btn btn-sm btn-ghost" to="/register" on:click={navClick}>
              <UserPlus size={14} />
              {$t('nav.register')}
            </Link>
          </div>
        {/if}

        <!-- Compact footer row: language + backend status -->
        <div class="footer-row">
          <div class="relative">
            <button class="foot-btn" on:click={() => langMenuOpen = !langMenuOpen} aria-label={$t('nav.language')}>
              <Globe size={13} />
              <span>{($locale || 'en').substring(0, 2).toUpperCase()}</span>
            </button>
            {#if langMenuOpen}
              <div class="lang-menu">
                <button on:click={() => setLocale('en')}>English</button>
                <button on:click={() => setLocale('nl')}>Nederlands</button>
              </div>
            {/if}
          </div>

          <button
            class="foot-btn"
            on:click={toggleTheme}
            title={$isDark ? $t('nav.switchToLight') : $t('nav.switchToDark')}
            aria-label={$t('nav.toggleTheme')}>
            {#if $isDark}<Sun size={13} />{:else}<Moon size={13} />{/if}
          </button>

          <div class="health-widget" class:health-widget-open={healthPopoverOpen}>
            <button
              bind:this={healthBtnRef}
              class="backend-dot"
              class:online={$backendHealth?.status === 'ok'}
              class:degraded={$backendHealth?.status === 'degraded'}
              class:offline={$backendHealth?.status === null && $backendHealth !== null}
              on:click={() => {
                healthPopoverOpen = !healthPopoverOpen;
                if (healthPopoverOpen) {
                  refreshHealth();
                  const r = healthBtnRef.getBoundingClientRect();
                  const popW = 290;
                  const left = Math.min(r.left, window.innerWidth - popW - 8);
                  healthPopoverStyle = `position:fixed;z-index:9999;bottom:${window.innerHeight - r.top + 8}px;left:${Math.max(8, left)}px`;
                }
              }}
              title={backendStatusLabel}
              aria-label={backendStatusLabel}
            >
              <span class="dot"></span>
            </button>
          </div>
        </div>
      </div>
    </aside>

    <main class="workspace-shell">
      <header class="topbar-shell">
        <div class="min-w-0">
          <div class="eyebrow">{currentPath === '/' ? $t('nav.overview') : $t('topbar.currentWorkspace')}</div>
          <h1 class="text-2xl md:text-3xl lg:text-4xl font-bold tracking-tight leading-none m-0">{pageMeta.title}</h1>
          <p class="mt-1.5 text-[var(--ink-700)] leading-relaxed max-w-2xl m-0">{pageMeta.detail}</p>
        </div>

        {#if searchEnabled}
          <div class="topbar-actions hidden lg:flex">
            <button
              class="search-trigger"
              on:click={() => { searchOpen = !searchOpen; if (searchOpen) setTimeout(() => searchBarRef?.focus(), 50); }}
            >
              <SearchIcon size={16} />
              <span class="flex-1 text-left">{$t('search.hint')}</span>
              <kbd>⌘K</kbd>
            </button>
          </div>
        {/if}
      </header>

      {#if searchOpen && searchEnabled}
        <!-- svelte-ignore a11y-click-events-have-key-events -->
        <!-- svelte-ignore a11y-no-static-element-interactions -->
        <div class="search-modal-overlay" on:click={() => searchOpen = false}>
          <div class="search-modal" on:click|stopPropagation role="dialog" aria-modal="true" aria-label={$t('system.search')} tabindex="-1">
            <div class="search-modal-header">
              <span class="search-modal-title">{$t('system.search')}</span>
              <button class="search-modal-close" on:click={() => searchOpen = false} aria-label={$t('nav.closeSearch')}>
                <X size={18} />
              </button>
            </div>
            <div class="search-modal-body">
              <SearchBar bind:this={searchBarRef} onclose={() => searchOpen = false} />
            </div>
          </div>
        </div>
      {/if}

      {#if $backendHealth !== null && $backendHealth.status === null}
        <div class="backend-banner" role="alert">
          <AlertTriangle size={16} />
          <span><strong>{$t('system.backendUnavailable')}</strong> <span class="banner-desc">{$t('system.backendUnavailableDesc')}</span></span>
          <button class="btn btn-sm btn-ghost" on:click={checkBackend}>
            <RefreshCw size={13} />
            {$t('system.retry')}
          </button>
        </div>
      {:else if $backendHealth?.status === 'degraded'}
        <div class="backend-banner backend-banner-warn" role="alert">
          <AlertTriangle size={16} />
          <span><strong>{$t('system.backendDegraded')}</strong> <span class="banner-desc">{$t('nav.degradedDesc')}</span></span>
          <button class="btn btn-sm btn-ghost" on:click={() => { healthPopoverOpen = true; checkBackend(); }}>
            <RefreshCw size={13} />
            {$t('nav.details')}
          </button>
        </div>
      {/if}

      <section class="page-shell">
        {#key $location?.pathname}
        <div class="route-view">
        <Route path="/" component={Home} />
        <Route path="/login" component={Login} />
        <Route path="/register" component={Register} />
        <Route path="/forgot-password" component={ForgotPassword} />
        <Route path="/reset-password" component={ResetPassword} />
        <Route path="/verify-email" component={VerifyEmail} />
        <Route path="/oauth/callback" component={OAuthCallback} />
        <Route path="/browse">
          <LazyPage loader={lazyTripleBrowser} />
        </Route>
        <Route path="/resource" component={ResourceDetail} />
        <Route path="/graphs" component={GraphList} />
        <Route path="/sparql">
          <LazyPage loader={lazySparqlEditor} />
        </Route>
        <Route path="/chat">
          <LazyPage loader={lazyLlmChat} />
        </Route>
        <Route path="/datasets/:id/sparql" let:params>
          <LazyPage loader={lazySparqlEditor} datasetId={params.id} />
        </Route>
        <Route path="/datasets/:id/api-services" let:params>
          <LazyPage loader={lazyApiServices} datasetId={params.id} />
        </Route>
        <Route path="/organisations/:id/api-services" let:params>
          <LazyPage loader={lazyApiServices} orgId={params.id} />
        </Route>
        <Route path="/groups/:id/api-services" let:params>
          <LazyPage loader={lazyApiServices} groupId={params.id} />
        </Route>
        <Route path="/organisations/:id/sparql" let:params>
          <LazyPage loader={lazySparqlEditor} orgId={params.id} />
        </Route>
        <Route path="/import">
          <LazyPage loader={lazyDataImport} />
        </Route>
        <!-- SHACL Studio: consolidated workspace. -->
        <Route path="/shacl">
          <LazyPage loader={lazyShaclStudio} />
        </Route>
        <Route path="/shacl/shapes">
          <LazyPage loader={lazyShapeLibrary} />
        </Route>
        <Route path="/shacl/shapes/:id" let:params>
          <LazyPage loader={lazyShapeGraphEditor} id={params.id} />
        </Route>
        <Route path="/shacl/pipelines">
          <LazyPage loader={lazyPipelinesList} />
        </Route>
        <!-- `:id` also captures "new" for the create flow; a separate /new route
             would double-match (the router renders every matching Route). -->
        <Route path="/shacl/pipelines/:id" let:params>
          <LazyPage loader={lazyPipelineEditor} id={params.id} />
        </Route>
        <!-- Phase 4 Results dashboard: combines pipeline + dataset runs. -->
        <Route path="/shacl/results">
          <LazyPage loader={lazyShaclResults} />
        </Route>
        <Route path="/validation" component={Validation} />
        <Route path="/datasets" component={Datasets} />
        <Route path="/datasets/:id/viewer" let:params>
          <LazyPage loader={lazyDatasetViewer} id={params.id} />
        </Route>
        <Route path="/datasets/:id/cesium" let:params>
          <LazyPage loader={lazyCesiumView} id={params.id} />
        </Route>
        <Route path="/datasets/:id" let:params>
          <DatasetDetail id={params.id} />
        </Route>
        <Route path="/organisations" component={Organisations} />
        <Route path="/organisations/:id" let:params>
          <OrgDetail id={params.id} />
        </Route>
        <Route path="/settings" component={Settings} />
        <Route path="/admin/users">
          <LazyPage loader={lazyAdminUsers} />
        </Route>
        <Route path="/admin/security">
          <LazyPage loader={lazyAdminSecurity} />
        </Route>
        <Route path="/admin/llm">
          <LazyPage loader={lazyAdminLlm} />
        </Route>
        <Route path="/admin/docs">
          <LazyPage loader={lazyDocEditor} />
        </Route>
        <Route path="/models">
          <LazyPage loader={lazyModelRegistry} />
        </Route>
        <Route path="/models/:id/viewer/:versionId" let:params>
          <LazyPage loader={lazyModelViewer} id={params.id} versionId={params.versionId} />
        </Route>
        <Route path="/models/:id/viewer" let:params>
          <LazyPage loader={lazyModelViewer} id={params.id} />
        </Route>
        <Route path="/models/:id/diff" let:params>
          <LazyPage loader={lazyModelDiff} id={params.id} />
        </Route>
        <Route path="/models/:id" let:params>
          <LazyPage loader={lazyModelDetail} id={params.id} />
        </Route>
        <Route path="/docs">
          <LazyPage loader={lazyDocumentation} />
        </Route>
        <Route path="/docs/:slug" let:params>
          <LazyPage loader={lazyDocumentation} slug={params.slug} />
        </Route>
        <Route path="/api-docs">
          <LazyPage loader={lazyApiDocs} />
        </Route>
        <Route path="/graph-viz">
          <LazyPage loader={lazyGraphVisualizer} />
        </Route>
        </div>
        {/key}
      </section>
    </main>

    <nav class="bottom-tabs sm:hidden" aria-label={$t('nav.quickNavigation')}>
      {#each BOTTOM_TABS as tab}
        <Link to={tab.to} class={`bottom-tab ${tab.match(currentPath) ? 'bottom-tab-active' : ''}`}>
          <svelte:component this={tab.icon} size={20} />
          <span class="text-[0.6rem] mt-0.5">{$t(tab.labelKey).split(' ')[0]}</span>
        </Link>
      {/each}
    </nav>
  </div>

  {#if healthPopoverOpen}
    <div bind:this={healthPopoverEl} class="health-popover" style={healthPopoverStyle} role="dialog" aria-label={$t('nav.serviceHealth')}>
      <div class="health-popover-head">
        <span class="health-popover-title">{$t('nav.serviceHealth')}</span>
        <button class="health-refresh-btn" class:spinning={healthRefreshing} on:click={refreshHealth} title={$t('system.refresh')} disabled={healthRefreshing}>
          <RefreshCw size={11} />
        </button>
      </div>
      {#if $backendHealth === null}
        <p class="health-checking">{$t('system.loading')}</p>
      {:else if $backendHealth.status === null}
        <p class="health-checking health-error">{$t('nav.serverUnreachable')}</p>
      {:else}
        {@const s = $backendHealth.services}
        <div class="health-rows">
          <div class="health-row">
            <span class="health-dot-sm" class:h-ok={s?.triplestore?.ok} class:h-err={!s?.triplestore?.ok}></span>
            <span class="health-label">Triplestore</span>
            {#if s?.triplestore?.ok}
              <span class="health-detail">ok</span>
            {:else}
              <span class="health-detail health-err-text">unavailable</span>
            {/if}
          </div>
          <div class="health-row">
            <span class="health-dot-sm" class:h-ok={s?.database?.ok} class:h-err={!s?.database?.ok}></span>
            <span class="health-label">{$t('nav.database')}</span>
            <span class="health-detail">{s?.database?.ok ? 'ok' : 'error'}</span>
          </div>
          <div class="health-row">
            <span class="health-dot-sm" class:h-ok={s?.object_storage?.configured} class:h-warn={!s?.object_storage?.configured}></span>
            <span class="health-label">{$t('nav.objectStorage')}</span>
            <span class="health-detail">{s?.object_storage?.configured ? 'configured' : 'not configured'}</span>
          </div>
          <div class="health-row">
            <span class="health-dot-sm" class:h-ok={s?.backup?.enabled} class:h-warn={!s?.backup?.enabled}></span>
            <span class="health-label">{$t('nav.backup')}</span>
            <span class="health-detail">{s?.backup?.enabled ? 'enabled' : 'disabled'}</span>
          </div>
        </div>
        {#if $backendHealth.version}
          <div class="health-version">v{$backendHealth.version}</div>
        {/if}
      {/if}
    </div>
  {/if}

  <Toasts />
</Router>

<!-- Global 3D-model / geometry preview, requested by RDF terms anywhere
     (triple table, graph explorer, resource panels). -->
<PreviewOverlay />
{/if}

<style>
  .app-shell {
    display: grid;
    grid-template-columns: 248px minmax(0, 1fr);
    min-height: 100vh;
    gap: 1.25rem;
    padding: 1.1rem;
  }

  .mobile-topbar { display: none; }
  .icon-btn { padding: 0.5rem; border-radius: 12px; background: transparent; border: none; color: inherit; cursor: pointer; transition: background 0.14s ease; }
  .icon-btn:hover { background: rgba(255,255,255,0.15); }
  .icon-btn-placeholder { width: 2.25rem; }

  .sidebar-shell {
    position: sticky;
    top: 1.1rem;
    align-self: start;
    display: flex;
    flex-direction: column;
    max-height: calc(100vh - 2.2rem);
    background: linear-gradient(180deg, rgba(21, 58, 67, 0.97), rgba(16, 46, 54, 0.97));
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 22px;
    box-shadow: var(--shadow-md);
    color: rgba(255, 255, 255, 0.95);
    overflow: hidden;
  }

  .app-loading {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: radial-gradient(120% 120% at 50% 35%, #14323b 0%, #0b2127 60%, #081519 100%);
    z-index: 9999;
  }

  .brand-block {
    padding: 1.1rem 1rem 0.85rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  }
  :global(.brand-link) { display: flex; flex-direction: column; align-items: flex-start; gap: 0.28rem; text-decoration: none; color: inherit; }
  :global(.brand-link-mobile) { display: flex; align-items: center; text-decoration: none; color: inherit; }

  /* The ring logo is the "O" of the wordmark — the whole mark (logo +
     "pen Triplestore") sits inside one rounded, gradient-framed tile. */
  .brand-word {
    display: inline-flex; align-items: center;
    font-weight: 700; font-size: 1.06rem; letter-spacing: 0.01em; line-height: 1; color: #fff;
    padding: 0.34em 0.6em 0.52em 0.34em;
    border-radius: 0.6em;
    background: linear-gradient(135deg, #1f5563, #0d2a32);
    box-shadow: inset 0 0 0 1px rgba(126, 214, 208, 0.3), 0 2px 8px rgba(13, 42, 50, 0.55);
  }
  .brand-word-sm { font-size: 1rem; }
  .brand-o {
    width: 1.65em; height: 1.65em; flex: 0 0 auto;
    margin: -0.3em 0.06em -0.3em 0;
    filter: drop-shadow(0 1px 3px rgba(13, 42, 50, 0.5));
    object-fit: contain; /* only applies when this class is on an <img> (operator-supplied logo) */
  }
  .brand-rest { display: inline-block; transform: translateY(0.08em); }
  .brand-sub {
    display: block;
    margin-top: 0.12rem;
    color: rgba(137, 207, 202, 0.85);
    font-size: 0.62rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.14em;
  }

  .sidebar-scroll {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0.75rem 0.75rem 0.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    scrollbar-width: thin;
    scrollbar-color: rgba(137, 207, 202, 0.6) rgba(0, 0, 0, 0.2);
  }

  /* The animated selector pill — slides between nav items on route change. */
  .nav-indicator {
    position: absolute;
    top: 0;
    left: 0;
    border-radius: 10px;
    background: linear-gradient(90deg, rgba(137, 207, 202, 0.18), rgba(255, 255, 255, 0.04));
    opacity: 0;
    pointer-events: none;
    z-index: 0;
    transition:
      transform 0.34s cubic-bezier(0.22, 1, 0.36, 1),
      width 0.34s cubic-bezier(0.22, 1, 0.36, 1),
      height 0.34s cubic-bezier(0.22, 1, 0.36, 1),
      opacity 0.2s ease;
  }
  .nav-indicator.visible { opacity: 1; }
  .nav-indicator::before {
    content: "";
    position: absolute;
    left: -0.75rem;
    top: 20%;
    bottom: 20%;
    width: 3px;
    background: linear-gradient(180deg, #7ED6D0, #2F7A8C);
    border-radius: 0 3px 3px 0;
  }
  @media (prefers-reduced-motion: reduce) {
    .nav-indicator { transition: opacity 0.2s ease; }
  }

  .sidebar-scroll::-webkit-scrollbar {
    width: 8px;
  }

  .sidebar-scroll::-webkit-scrollbar-track {
    background: rgba(0, 0, 0, 0.2);
    border-radius: 4px;
  }

  .sidebar-scroll::-webkit-scrollbar-thumb {
    background: linear-gradient(180deg, #89cfca, #5b8d54);
    border-radius: 4px;
    border: 2px solid rgba(0, 0, 0, 0.2);
  }

  .sidebar-scroll::-webkit-scrollbar-thumb:hover {
    background: linear-gradient(180deg, #7ed6d0, #7a9d6f);
  }

  .sidebar-section { display: flex; flex-direction: column; gap: 0.35rem; }
  .sidebar-heading { padding: 0 0.6rem; color: rgba(255, 255, 255, 0.42); font-size: 0.66rem; font-weight: 700; letter-spacing: 0.14em; text-transform: uppercase; }

  .nav-group { display: flex; flex-direction: column; gap: 0.1rem; }

  :global(.nav-item) {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    padding: 0.52rem 0.65rem;
    border-radius: 10px;
    text-decoration: none;
    color: rgba(255, 255, 255, 0.78);
    font-size: 0.88rem;
    font-weight: 500;
    transition: background 0.14s ease, color 0.14s ease;
    position: relative;
    z-index: 1;
  }
  :global(.nav-item:hover) { background: rgba(255, 255, 255, 0.06); color: white; }
  :global(.nav-item.selected) {
    color: white;
    font-weight: 600;
  }
  :global(.nav-item-label) { font-size: 0.88rem; }

  .sidebar-footer {
    border-top: 1px solid rgba(255,255,255,0.06);
    padding: 0.7rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    background: rgba(0,0,0,0.12);
  }

  .user-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.55rem 0.65rem;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 12px;
  }
  .user-meta { display: flex; align-items: center; gap: 0.55rem; min-width: 0; }
  .user-text { min-width: 0; }
  .user-text strong { display: block; font-size: 0.82rem; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 8.5rem; }
  .user-text small { display: block; margin-top: 0.05rem; color: rgba(255, 255, 255, 0.5); font-size: 0.7rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 8.5rem; }
  .user-avatar { display: grid; place-items: center; width: 1.9rem; height: 1.9rem; border-radius: 50%; background: linear-gradient(135deg, #7ED6D0, #2F7A8C); font-weight: 700; font-size: 0.82rem; flex-shrink: 0; }
  .user-actions { display: flex; align-items: center; gap: 0.25rem; }
  .icon-link { display: grid; place-items: center; width: 1.9rem; height: 1.9rem; border-radius: 8px; background: transparent; border: none; color: rgba(255,255,255,0.65); cursor: pointer; text-decoration: none; transition: background 0.14s ease, color 0.14s ease; }
  .icon-link:hover { background: rgba(255,255,255,0.1); color: white; }

  .auth-actions { display: flex; gap: 0.45rem; }
  .auth-actions :global(.btn) { flex: 1; }

  .footer-row { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }
  .foot-btn { display: flex; align-items: center; gap: 0.3rem; padding: 0.3rem 0.5rem; border-radius: 8px; background: transparent; border: none; color: rgba(255,255,255,0.55); font-size: 0.7rem; font-weight: 600; letter-spacing: 0.05em; cursor: pointer; transition: background 0.14s ease, color 0.14s ease; }
  .foot-btn:hover { background: rgba(255,255,255,0.08); color: white; }
  .lang-menu { position: absolute; bottom: calc(100% + 4px); left: 0; min-width: 120px; background: #1a4450; border: 1px solid rgba(255,255,255,0.1); border-radius: 10px; overflow: hidden; box-shadow: var(--shadow-md); z-index: 20; }
  .lang-menu button { display: block; width: 100%; text-align: left; padding: 0.5rem 0.75rem; background: transparent; border: none; color: rgba(255,255,255,0.82); font-size: 0.8rem; cursor: pointer; }
  .lang-menu button:hover { background: rgba(255,255,255,0.08); }

  .backend-dot {
    display: inline-grid;
    place-items: center;
    width: 1.6rem;
    height: 1.6rem;
    border-radius: 50%;
    background: transparent;
    border: none;
    cursor: pointer;
    transition: background 0.14s ease;
  }
  .backend-dot:hover { background: rgba(255,255,255,0.08); }
  .backend-dot .dot { width: 0.5rem; height: 0.5rem; border-radius: 50%; background: #cdd6dc; transition: background 0.14s ease, box-shadow 0.14s ease; }
  .backend-dot.online .dot { background: #94d38d; box-shadow: 0 0 6px rgba(148, 211, 141, 0.65); }
  .backend-dot.degraded .dot { background: #f5c842; box-shadow: 0 0 6px rgba(245, 200, 66, 0.65); }
  .backend-dot.offline .dot { background: #ef9e8a; box-shadow: 0 0 6px rgba(239, 158, 138, 0.6); }

  /* ── Health popover ──────────────────────────────────────────────────────── */
  .health-widget { position: relative; }

  .health-popover {
    position: fixed;
    width: 290px;
    background: #1e2530;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 12px;
    box-shadow: 0 8px 32px rgba(0,0,0,0.45);
    z-index: 9999;
    overflow: hidden;
    color: #e2e8f0;
    font-size: 0.8rem;
  }

  .health-popover-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.55rem 0.75rem 0.4rem;
    border-bottom: 1px solid rgba(255,255,255,0.08);
  }
  .health-popover-title {
    font-size: 0.7rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: #94a3b8;
  }
  .health-refresh-btn {
    background: none;
    border: none;
    color: #64748b;
    cursor: pointer;
    padding: 0.1rem;
    border-radius: 4px;
    line-height: 1;
    display: flex;
    align-items: center;
  }
  .health-refresh-btn:hover { color: #94d38d; }
  .health-refresh-btn.spinning { color: #94d38d; cursor: default; }
  @keyframes spin { to { transform: rotate(360deg); } }

  .health-checking { padding: 0.75rem; margin: 0; color: #94a3b8; font-size: 0.8rem; }
  .health-error { color: #ef9e8a; }

  .health-rows { display: flex; flex-direction: column; padding: 0.3rem 0; }
  .health-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.75rem;
    min-width: 0;
  }
  .health-row:hover { background: rgba(255,255,255,0.04); }

  .health-dot-sm {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    background: #475569;
  }
  .health-dot-sm.h-ok { background: #94d38d; box-shadow: 0 0 4px rgba(148,211,141,0.5); }
  .health-dot-sm.h-err { background: #ef9e8a; box-shadow: 0 0 4px rgba(239,158,138,0.5); }
  .health-dot-sm.h-warn { background: #f5c842; box-shadow: 0 0 4px rgba(245,200,66,0.5); }

  .health-label { font-size: 0.8rem; color: #cbd5e1; flex: 1; white-space: nowrap; }
  .health-detail { font-size: 0.73rem; color: #64748b; text-align: right; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 160px; }
  .health-err-text { color: #ef9e8a; }

  .health-version {
    padding: 0.3rem 0.75rem 0.45rem;
    border-top: 1px solid rgba(255,255,255,0.06);
    font-size: 0.68rem;
    color: #475569;
    text-align: right;
  }

  .workspace-shell { min-width: 0; display: flex; flex-direction: column; gap: 1rem; }

  .topbar-shell {
    display: flex;
    align-items: end;
    justify-content: space-between;
    gap: 1rem;
    padding: 1.25rem 1.4rem;
    border: 1px solid var(--line-soft);
    border-radius: 24px;
    background: rgba(255, 252, 248, 0.78);
    backdrop-filter: blur(16px);
    box-shadow: var(--shadow-sm);
  }
  .topbar-actions { align-items: center; gap: 0.65rem; }

  .search-trigger {
    display: flex; align-items: center; gap: 0.5rem;
    padding: 0.6rem 0.9rem;
    border-radius: 12px;
    background: rgba(255,255,255,0.6);
    border: 1px solid var(--line-soft);
    color: var(--ink-500);
    font-size: 0.85rem;
    cursor: pointer;
    min-width: 260px;
    transition: background 0.14s ease, border-color 0.14s ease;
  }
  .search-trigger:hover { background: white; border-color: var(--brand-300); }
  .search-trigger kbd { padding: 0.1rem 0.35rem; font-size: 0.65rem; font-family: inherit; border: 1px solid var(--line-soft); border-radius: 6px; background: rgba(255,255,255,0.9); }

  .backend-banner {
    display: flex; align-items: center; gap: 0.6rem;
    padding: 0.55rem 0.9rem;
    border-radius: 12px;
    border: 1px solid rgba(245, 158, 11, 0.35);
    background: rgba(254, 243, 199, 0.5);
    color: #78350f;
    font-size: 0.82rem;
  }
  .backend-banner .banner-desc { color: rgba(120, 53, 15, 0.8); margin-left: 0.25rem; }
  .backend-banner .btn { margin-left: auto; }
  .backend-banner-warn {
    border-color: rgba(245, 200, 66, 0.4);
    background: rgba(255, 251, 230, 0.5);
    color: #78600f;
  }

  .page-shell { min-width: 0; padding-bottom: 1rem; }

  .sidebar-overlay { display: none; position: fixed; inset: 0; background: rgba(0, 0, 0, 0.4); z-index: 40; border: none; cursor: default; }

  /* ─── Search modal ─────────────────────────────────────────────────────── */
  .search-modal-overlay {
    position: fixed; inset: 0; z-index: 200;
    background: rgba(0, 0, 0, 0.45);
    backdrop-filter: blur(4px);
    display: flex; align-items: flex-start; justify-content: center;
    padding: 6vh 1rem 1rem;
  }
  .search-modal {
    width: 100%; max-width: 620px;
    background: #fff;
    border-radius: 16px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.22);
    overflow: hidden;
    display: flex; flex-direction: column;
    max-height: 85vh;
  }
  .search-modal-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.85rem 1.1rem 0.75rem;
    border-bottom: 1px solid var(--line-soft, #e2e8f0);
    flex-shrink: 0;
  }
  .search-modal-title {
    font-size: 0.78rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--ink-400, #94a3b8);
  }
  .search-modal-close {
    display: flex; align-items: center; justify-content: center;
    width: 30px; height: 30px;
    border: none; background: transparent; cursor: pointer;
    color: var(--ink-400, #94a3b8); border-radius: 6px;
    transition: background 0.12s, color 0.12s;
  }
  .search-modal-close:hover { background: var(--line-soft, #f1f5f9); color: var(--ink-700, #334155); }
  .search-modal-body {
    padding: 1rem 1.1rem 1.25rem;
    overflow-y: auto;
  }

  .bottom-tabs { display: none; }
  :global(.bottom-tab) { display: flex; flex-direction: column; align-items: center; justify-content: center; flex: 1; padding: 0.5rem 0; text-decoration: none; color: var(--ink-500); transition: color 0.14s ease; }
  :global(.bottom-tab-active) { color: var(--brand-500); }

  @media (max-width: 1024px) {
    .app-shell { grid-template-columns: 1fr; padding-top: 4rem; }
    .mobile-topbar {
      display: flex; position: fixed; top: 0; left: 0; right: 0; z-index: 50;
      align-items: center; justify-content: space-between;
      padding: 0.6rem 1rem;
      background: rgba(21, 58, 67, 0.95);
      backdrop-filter: blur(16px); color: white;
    }
    .sidebar-shell {
      position: fixed; top: 3.5rem; left: 0; bottom: 0; z-index: 45;
      width: 280px; height: calc(100vh - 3.5rem); max-height: none;
      border-radius: 0;
      background: linear-gradient(180deg, rgb(21, 58, 67), rgb(16, 46, 54));
      transform: translateX(-100%);
      transition: transform 0.25s cubic-bezier(0.4, 0, 0.2, 1);
    }
    .sidebar-shell.sidebar-open { transform: translateX(0); }
    .sidebar-overlay { display: block; top: 3.5rem; }
    .topbar-shell { flex-direction: column; align-items: stretch; }
  }

  @media (max-width: 640px) {
    .app-shell { padding: 0.75rem; padding-top: 4.5rem; padding-bottom: 4.5rem; gap: 0.75rem; }
    .topbar-shell { border-radius: 18px; padding: 1rem; }
    .bottom-tabs {
      display: flex; position: fixed; bottom: 0; left: 0; right: 0; z-index: 50;
      background: rgba(255, 252, 248, 0.95);
      backdrop-filter: blur(16px);
      border-top: 1px solid var(--line-soft);
      padding: 0.25rem 0;
      padding-bottom: env(safe-area-inset-bottom, 0.25rem);
    }
  }

  /* ── Dark theme: shell surfaces that hardcode light colours ───────────────── */
  :global(html.dark) .topbar-shell {
    background: rgba(15, 23, 40, 0.72);
    border-color: var(--line-soft);
  }
  :global(html.dark) .search-trigger {
    background: rgba(255, 255, 255, 0.05);
    border-color: var(--line-soft);
    color: var(--ink-500);
  }
  :global(html.dark) .search-trigger:hover {
    background: rgba(255, 255, 255, 0.09);
    border-color: var(--brand-400);
  }
  :global(html.dark) .search-trigger kbd {
    background: rgba(255, 255, 255, 0.06);
    border-color: var(--line-soft);
  }
  :global(html.dark) .search-modal {
    background: var(--bg-strong);
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.6);
  }
  :global(html.dark) .search-modal-close:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--ink-700);
  }
  :global(html.dark) .backend-banner {
    background: rgba(245, 158, 11, 0.12);
    border-color: rgba(245, 158, 11, 0.38);
    color: #fbdca0;
  }
  :global(html.dark) .backend-banner .banner-desc { color: rgba(251, 220, 160, 0.8); }
  :global(html.dark) .backend-banner-warn {
    background: rgba(245, 200, 66, 0.12);
    border-color: rgba(245, 200, 66, 0.38);
    color: #f3dca2;
  }
  :global(html.dark) .bottom-tabs {
    background: rgba(11, 18, 32, 0.95);
    border-top-color: var(--line-soft);
  }
</style>
