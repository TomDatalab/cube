import {
  Badge,
  Button,
  DialogContainer,
  Flex,
  Radio,
  Panel,
  SearchInput,
  Space,
  tasty,
  Text,
  Title,
  TooltipProvider,
  ResizablePanel,
  ClearIcon,
  CubeIcon,
  ViewIcon,
} from '@cube-dev/ui-kit';
import {
  ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  DatabaseOutlined,
  EditOutlined,
  LoadingOutlined,
  StarFilled,
  StarOutlined,
  WarningOutlined,
} from '@ant-design/icons';
import { Select } from 'antd';

import { useRbac } from '../hooks/rbac';
import { useWorkspace } from '../hooks/workspace';
import { chooseOrganizationScope } from '../shared/auth';
import {
  otherScopeSections, rememberCubeToOpen, ScopeSection, sectionsOf, takeCubeToOpen, worthGrouping,
} from '../shared/meta-groups';

import {
  useAllScopeGroups, useDebouncedValue, useFilteredCubes, useEvent, useLocalStorage, useMetaGroups,
} from './hooks';
import { useQueryBuilderContext } from './context';
import { EditQueryDialogForm } from './components/EditQueryDialogForm';
import { SidePanelCubeItem } from './components/SidePanelCubeItem';
import { validateQuery } from './utils';
import { ChevronIcon } from './icons/ChevronIcon';

const DEFAULT_SIDEBAR_SIZE = 315;

const RadioButton = tasty(Radio.Button, {
  styles: { flexGrow: 1, placeItems: 'stretch' },
  inputStyles: { textAlign: 'center' },
});

const CountBadge = tasty(Badge, {
  styles: {
    fill: '#purple',
    border: '#purple',
    color: '#white',
    padding: '0 1ow',
  },
});

// An organization's header in the cube list: opens and closes its cubes.
const GroupButton = tasty(Button, {
  qa: 'OrganizationGroup',
  type: 'secondary',
  size: 'small',
  styles: {
    radius: 0,
    border: '#clear',
    color: '#dark',
    fill: {
      '': '#dark.04',
      hovered: '#dark.08',
      pressed: '#dark.06',
    },
    padding: '.75x 1x',
    display: 'grid',
    gridTemplateColumns: '16px 1fr auto auto',
    gap: '1x',
    placeItems: 'center start',
    textAlign: 'left',
    preset: 't3m',
  },
});

const DataSourceLabel = tasty(Text, {
  qa: 'DataSourceGroup',
  preset: 'c2',
  styles: {
    display: 'flex',
    gap: '.5x',
    placeItems: 'center',
    color: '#dark-03',
    padding: '.5x 1x .25x 2x',
  },
});

// A cube or view of an organization whose model is not the current one.
const OtherScopeButton = tasty(Button, {
  qa: 'OtherScopeCube',
  type: 'secondary',
  size: 'small',
  styles: {
    radius: 0,
    border: '#clear',
    color: '#dark-03',
    fill: {
      '': '#clear',
      hovered: '#purple.08',
      pressed: '#purple.12',
    },
    padding: '.75x 1x',
    display: 'grid',
    gridTemplateColumns: '16px 1fr',
    gap: '1x',
    placeItems: 'center start',
    textAlign: 'left',
    preset: 't3',
  },
});

const ORGANIZATION_FILTER_KEY = 'QueryBuilder:Sidebar:organization';

// Where a section sorts: by its place among every scope's groups; one they
// do not name (Other) last.
function orderOf(order: string[], slug: string): number {
  const at = order.indexOf(slug);
  return at === -1 ? order.length : at;
}

type Props = {
  defaultSelectedType?: 'cubes' | 'views';
  customTypeSwitcher?: ReactNode;
  showEditQueryButton?: boolean;
  width?: string;
};

export function QueryBuilderSidePanel({
  defaultSelectedType = 'cubes',
  customTypeSwitcher = null,
  showEditQueryButton = true,
  width,
}: Props) {
  const {
    query,
    cubes: cubesAndViews = [],
    selectCube,
    isQueryEmpty,
    joinableCubes,
    isCubeUsed,
    meta,
    isVerifying,
    clearQuery,
    setQuery,
    usedCubes,
    usedMembers,
    queryStats,
    members,
    missingCubes,
    apiVersion,
    isMetaLoading,
    memberViewType,
    disableSidebarResizing,
    apiUrl,
    apiToken,
  } = useQueryBuilderContext();

  // The cubes by organization and data source, as the Data Model page shows
  // them; `null` when the API cannot say (the list is then flat).
  const metaGroups = useMetaGroups(apiUrl, apiToken, meta);
  // A platform admin outside a workspace sees every organization: the
  // current model's cubes to query, the others' by name, each opening in its
  // organization's model (a query reads one model).
  const rbac = useRbac();
  const workspace = useWorkspace();
  const allScopeGroups = useAllScopeGroups(
    apiUrl,
    rbac.enabled && Boolean(rbac.session?.user.platformAdmin) && !workspace,
    meta
  );
  const [organizationFilter, setOrganizationFilter] = useLocalStorage<string>(
    ORGANIZATION_FILTER_KEY,
    ''
  );
  const [closedGroups, setClosedGroups] = useState<Set<string>>(new Set());

  const contentRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [scrollToCubeName, setScrollToCubeName] = useState<string | null>(null);

  const [viewMode, setViewMode] = useState<'all' | 'query'>(!usedCubes.length ? 'all' : 'query');
  const [isPasteDialogOpen, setIsPasteDialogOpen] = useState(false);
  const [filterString, setFilterString] = useState('');

  const [selectedType, setSelectedType] = useState<'cubes' | 'views'>(defaultSelectedType);

  const [sidebarSize, setSidebarSize] = useLocalStorage(
    'QueryBuilder:Sidebar:size',
    DEFAULT_SIDEBAR_SIZE
  );

  const cubes = cubesAndViews.filter((item) => item.type === 'cube');
  const views = cubesAndViews.filter((item) => item.type === 'view');

  const preparedFilterString = filterString.trim().toLowerCase();
  const debouncedFilterString = useDebouncedValue(preparedFilterString, 500);
  const appliedFilterString = preparedFilterString.length < 2 ? '' : debouncedFilterString;

  const cubesOrViews = selectedType === 'cubes' ? cubes : views;
  const allJoinableCubes = selectedType === 'views' && usedCubes.length
    ? cubesOrViews.filter((cube) => usedCubes[0] === cube.name)
    : cubesOrViews.filter((cube) => joinableCubes.includes(cube));

  const [openCubes, setOpenCubes] = useState<Set<string>>(new Set());

  useLayoutEffect(() => {
    if (isQueryEmpty) {
      setOpenCubes(cubesOrViews.length === 1 ? new Set([cubesOrViews[0].name]) : new Set());
    }
  }, [cubesOrViews.length, selectedType]);

  const highlightedCubes = appliedFilterString ? usedCubes : [];

  cubesOrViews.sort((a, b) => {
    if (highlightedCubes.includes(a.name) && !highlightedCubes.includes(b.name)) {
      return -1;
    }

    if (!highlightedCubes.includes(a.name) && highlightedCubes.includes(b.name)) {
      return 1;
    }

    return memberViewType === 'name'
      ? a.name.localeCompare(b.name)
      : a.title.localeCompare(b.title);
  });

  // Filtered cubes
  const filteredCubes = useFilteredCubes(
    appliedFilterString,
    allJoinableCubes,
    memberViewType
  ).cubes.map((cube) => cube.name);

  const resetScrollAndContentSize = useCallback(() => {
    if (contentRef?.current) {
      const element = contentRef.current;

      element.scrollTop = 0;

      setTimeout(() => {
        element.scrollTop = 0;
      }, 0);
    }
  }, [contentRef?.current]);

  useEffect(() => {
    resetScrollAndContentSize();

    if (appliedFilterString && viewMode === 'query') {
      setViewMode('all');
    }
  }, [selectedType, appliedFilterString, meta]);

  useEffect(() => {
    const usedView = views.find((cube) => isCubeUsed(cube.name));
    const usedCube = cubes.find((cube) => isCubeUsed(cube.name));

    if (selectedType === 'cubes' && usedView && !usedCube) {
      setSelectedType('views');
      selectCube(usedView?.name);
    } else if (selectedType === 'views' && usedCube) {
      setSelectedType('cubes');
    }
  }, [selectedType, query]);

  const switchType = useCallback(
    async (type: 'cubes' | 'views') => {
      setSelectedType(type);
    },
    [isQueryEmpty]
  );

  const editQueryButton = useMemo(
    () => (
      <Button
        qa="EditQueryButton"
        aria-label="Edit Query"
        type="primary"
        size="small"
        icon={<EditOutlined />}
        onPress={() => setIsPasteDialogOpen(true)}
      />
    ),
    []
  );

  const typeSwitcher = useMemo(() => (
    <Space qa="QueryBuilderSwitcher" gap="1x">
      {editQueryButton}
      <Radio.ButtonGroup
        aria-label="Cube type"
        value={selectedType}
        styles={{ flexGrow: 1 }}
        onChange={(val) => switchType(val as 'cubes' | 'views')}
      >
        <RadioButton
          qa="QueryBuilderTab-cubes"
          value="cubes"
          isDisabled={!cubes.length}
          inputStyles={{ placeContent: 'center' }}
        >
          Cubes <CountBadge radius="1r">{cubes.length}</CountBadge>
        </RadioButton>
        <RadioButton
          qa="QueryBuilderTab-views"
          value="views"
          isDisabled={!views.length}
          inputStyles={{ placeContent: 'center' }}
        >
          Views <CountBadge radius="1r">{views.length}</CountBadge>
        </RadioButton>
      </Radio.ButtonGroup>
    </Space>
  ),
  [selectedType, meta, cubes.length, views.length]);

  // Back from switching to another organization's model to query one of
  // its cubes: open that cube, on its tab, once the meta has it.
  useEffect(() => {
    if (!cubesAndViews.length) {
      return;
    }
    const name = takeCubeToOpen();
    const item = name ? cubesAndViews.find((c) => c.name === name) : undefined;
    if (!item) {
      return;
    }
    setSelectedType(item.type === 'view' ? 'views' : 'cubes');
    setViewMode('all');
    setOpenCubes((open) => new Set([...open, item.name]));
    setScrollToCubeName(item.name);
  }, [cubesAndViews.length]);

  const searchInput = useMemo(() => {
    const description = `Search ${selectedType === 'cubes' ? 'cubes' : 'views'} and members`;

    return (
      <SearchInput
        isClearable
        qa="QueryBuilderSearch"
        size="small"
        aria-label={description}
        placeholder={description}
        value={filterString}
        onChange={(val) => setFilterString(val)}
      />
    );
  }, [selectedType, meta, filterString]);

  useEffect(() => {
    if (scrollToCubeName) {
      setTimeout(() => {
        const element = containerRef.current?.querySelector(
          `[data-qa="CubeButton"][data-qaval="${scrollToCubeName}"]`
        );

        if (element) {
          element.scrollIntoView({
            block: 'start',
          });
        }
      });

      setScrollToCubeName(null);
    }
  }, [scrollToCubeName]);

  // Close all disabled cubes to avoid layout shift on deselecting member.
  useEffect(() => {
    const currentSize = openCubes.size;
    const allJoinableCubeNames = allJoinableCubes.map((cube) => cube.name);

    openCubes.forEach((cubeName) => {
      if (!allJoinableCubeNames.includes(cubeName) && !missingCubes.includes(cubeName)) {
        openCubes.delete(cubeName);
      }
    });

    if (currentSize !== openCubes.size) {
      setOpenCubes(new Set(openCubes));
    }
  }, [openCubes.size, missingCubes.length, allJoinableCubes.length]);

  function resetState(cubeName?: string) {
    setFilterString('');
    setViewMode('all');

    if (cubeName) {
      setOpenCubes(new Set([cubeName]));
      setScrollToCubeName(cubeName);
    }
  }

  function onCubeToggle(name: string, isOpen: boolean) {
    if (appliedFilterString || viewMode === 'query') {
      resetState(name);

      return;
    }

    if (isOpen) {
      openCubes.add(name);
    } else {
      openCubes.delete(name);
    }

    setOpenCubes(new Set(openCubes));
  }

  const onMemberToggle = useEvent((cubeName: string, memberName: string) => {
    const isTimeDimension = members.dimensions[memberName]?.type === 'time';

    // Always reset state if we click on time dimension
    if (isTimeDimension || (appliedFilterString && !usedMembers.includes(memberName))) {
      resetState(cubeName);
    }
  });

  const onHierarchyToggle = useEvent((cubeName?: string) => {
    if (appliedFilterString || viewMode === 'query') {
      resetState(cubeName);
    }
  });

  // Every section of the current tab, for the filter and to decide whether
  // grouping says anything. An organization with nothing to query but errors
  // (only admins get error counts) keeps its section, so they see it.
  const allSections = metaGroups
    ? sectionsOf(cubesOrViews, metaGroups, { keepEmpty: true })
      .filter((section) => section.items.length || (section.errors ?? 0) > 0)
    : [];
  const allOtherSections = allScopeGroups && metaGroups
    ? otherScopeSections(allScopeGroups, metaGroups, selectedType)
    : [];
  const isGrouped = worthGrouping(allSections) || allOtherSections.length > 0;
  // The organizations in the order of every scope's groups (Platform first),
  // then whatever only the current model has (Other).
  const sectionOrder = (allScopeGroups || []).map((group) => group.slug);
  const filterSections = [
    ...allSections.map((section) => ({ slug: section.slug, name: section.name, count: section.items.length })),
    ...allOtherSections,
  ].sort((a, b) => orderOf(sectionOrder, a.slug) - orderOf(sectionOrder, b.slug));
  const activeOrganization = isGrouped
    && filterSections.some((section) => section.slug === organizationFilter)
    ? organizationFilter
    : '';

  const organizationSelect = isGrouped && filterSections.length > 1 ? (
    <Select
      size="small"
      data-qa="OrganizationFilter"
      aria-label="Organization"
      value={activeOrganization}
      style={{ width: '100%' }}
      options={[
        { value: '', label: 'All organizations' },
        ...filterSections.map((section) => ({
          value: section.slug,
          label: `${section.name} (${section.count})`,
        })),
      ]}
      onChange={(value: string) => setOrganizationFilter(value)}
    />
  ) : null;

  function toggleGroup(slug: string) {
    const next = new Set(closedGroups);
    if (next.has(slug)) {
      next.delete(slug);
    } else {
      next.add(slug);
    }
    setClosedGroups(next);
  }

  const cubeList = useMemo(() => {
    const shownCubes = cubesOrViews.filter((cube) => {
      // If filter is applied, show only filtered cubes
      if (appliedFilterString) {
        return filteredCubes.includes(cube.name);
      }

      // In query mode, show only used cubes
      if (viewMode === 'query') {
        return usedCubes.includes(cube.name);
      }

      return true;
    });

    const renderCube = (cube: (typeof cubesOrViews)[number]) => (
      <SidePanelCubeItem
        key={cube.name}
        isNonJoinable={!allJoinableCubes.includes(cube) && !usedCubes.includes(cube.name)}
        isOpen={openCubes.has(cube.name)}
        filterString={appliedFilterString}
        cubeName={cube.name}
        mode={viewMode}
        rightIcon={isQueryEmpty ? 'arrow' : 'plus'}
        onToggle={(isOpen) => {
          onCubeToggle(cube.name, isOpen);
        }}
        onMemberToggle={(name) => {
          onMemberToggle(cube.name, name);
        }}
        onHierarchyToggle={onHierarchyToggle}
      />
    );

    // Searching or showing the query's cubes: every group open, and only
    // the ones with something to show.
    const narrowed = Boolean(appliedFilterString) || viewMode === 'query';
    const sections = isGrouped && metaGroups
      ? sectionsOf(shownCubes, metaGroups, { keepEmpty: true }).filter((section) => (
        (!activeOrganization || section.slug === activeOrganization)
        && (section.items.length || (!narrowed && (section.errors ?? 0) > 0))
      ))
      : null;
    // Another model's cubes are not in the query, so "query" mode shows none.
    const otherSections = allScopeGroups && metaGroups && viewMode !== 'query'
      ? otherScopeSections(allScopeGroups, metaGroups, selectedType, appliedFilterString)
        .filter((section) => !activeOrganization || section.slug === activeOrganization)
      : [];

    const renderErrors = (errors: number) => (errors > 0 ? (
      <TooltipProvider
        title={`${errors} problem${errors === 1 ? '' : 's'} in this organization's model: its rejected cubes are left out. See Data Model.`}
      >
        <Text color="#danger-text" aria-label={`${errors} problems`}>
          <WarningOutlined /> {errors}
        </Text>
      </TooltipProvider>
    ) : <span />);

    const renderOtherSection = (section: ScopeSection) => {
      const isOpen = narrowed || !closedGroups.has(section.slug);
      const Icon = selectedType === 'views' ? ViewIcon : CubeIcon;
      return (
        <Flex key={section.slug} gap="1bw" flow="column" data-qa="OrganizationSection" data-qaval={section.slug}>
          <GroupButton
            qaVal={section.slug}
            aria-expanded={isOpen}
            onPress={() => toggleGroup(section.slug)}
          >
            <ChevronIcon direction={isOpen ? 'bottom' : 'right'} />
            <Text ellipsis preset="t3m" title={section.name}>{section.name}</Text>
            {renderErrors(section.errors ?? 0)}
            <CountBadge radius="1r">{section.count}</CountBadge>
          </GroupButton>
          {isOpen && !section.count ? (
            <DataSourceLabel>Nothing to query here</DataSourceLabel>
          ) : null}
          {isOpen ? section.dataSources.map((ds) => (
            <Flex key={ds.name} gap="1bw" flow="column">
              {ds.name && selectedType === 'cubes' ? (
                <DataSourceLabel title={`Data source ${ds.name}`}>
                  <DatabaseOutlined /> {ds.name} · {ds.names.length}
                </DataSourceLabel>
              ) : null}
              {ds.names.map((name) => (
                <TooltipProvider
                  key={name}
                  title={`Opens ${section.name}'s model to query ${name}`}
                  placement="right"
                >
                  <OtherScopeButton
                    qaVal={name}
                    onPress={() => {
                      rememberCubeToOpen(name);
                      chooseOrganizationScope(section.slug);
                    }}
                  >
                    <Icon color="#dark-03" />
                    <Text ellipsis>{name}</Text>
                  </OtherScopeButton>
                </TooltipProvider>
              ))}
            </Flex>
          )) : null}
        </Flex>
      );
    };

    const renderSection = (section: NonNullable<typeof sections>[number]) => {
      const isOpen = narrowed || !closedGroups.has(section.slug);
      const errors = section.errors ?? 0;
      return (
        <Flex key={section.key} gap="1bw" flow="column" data-qa="OrganizationSection" data-qaval={section.slug}>
          <GroupButton
            qaVal={section.slug}
            aria-expanded={isOpen}
            onPress={() => toggleGroup(section.slug)}
          >
            <ChevronIcon direction={isOpen ? 'bottom' : 'right'} />
            <Text ellipsis preset="t3m" title={section.name}>{section.name}</Text>
            {renderErrors(errors)}
            <CountBadge radius="1r">{section.items.length}</CountBadge>
          </GroupButton>
          {isOpen && !section.items.length ? (
            <DataSourceLabel>Nothing to query here</DataSourceLabel>
          ) : null}
          {isOpen && selectedType === 'cubes'
            ? section.dataSources.map((ds) => (
              <Flex key={ds.name} gap="1bw" flow="column">
                {ds.name ? (
                  <DataSourceLabel title={`Data source ${ds.name}`}>
                    <DatabaseOutlined /> {ds.name} · {ds.items.length}
                  </DataSourceLabel>
                ) : null}
                {ds.items.map(renderCube)}
              </Flex>
            ))
            : null}
          {isOpen && selectedType === 'views' ? section.items.map(renderCube) : null}
        </Flex>
      );
    };

    // The current model's sections and the other organizations' in one
    // order: Platform, then the organizations by name, then Other.
    const ordered = sections
      ? [
        ...sections.map((section) => ({ slug: section.slug, node: renderSection(section) })),
        ...otherSections.map((section) => ({ slug: section.slug, node: renderOtherSection(section) })),
      ].sort((a, b) => orderOf(sectionOrder, a.slug) - orderOf(sectionOrder, b.slug))
      : null;

    return (
      <Flex gap="1bw" flow="column" padding="0 0 2x 0">
        {missingCubes
          .filter((cubeName) => (appliedFilterString ? filteredCubes.includes(cubeName) : true))
          .map((cubeName) => (
            <SidePanelCubeItem
              key={cubeName}
              isOpen={openCubes.has(cubeName)}
              filterString={appliedFilterString}
              cubeName={cubeName}
              mode={viewMode}
              rightIcon="arrow"
              onHierarchyToggle={onHierarchyToggle}
              onMemberToggle={(name) => {
                onMemberToggle(cubeName, name);
              }}
            />
          ))}
        {ordered
          ? ordered.map((entry) => entry.node)
          : shownCubes.map(renderCube)}
      </Flex>
    );
  },
  [
    viewMode,
    queryStats,
    [...openCubes.values()].join(),
    appliedFilterString,
    memberViewType,
    selectedType,
    metaGroups,
    allScopeGroups,
    isGrouped,
    activeOrganization,
    [...closedGroups.values()].join(),
  ]);

  const onApplyQuery = useCallback(async (appliedQuery) => {
    try {
      const validatedQuery = validateQuery(appliedQuery);

      setQuery(validatedQuery);
    } catch {
      throw new Error('Invalid query');
    }
  }, []);

  useEffect(() => {
    if (viewMode === 'query') {
      if (filterString) {
        setFilterString('');
      }

      setOpenCubes(new Set(usedCubes));

      setScrollToCubeName(usedCubes[0]);

      if (isQueryEmpty) {
        setViewMode('all');
      }
    }
  }, [viewMode, isQueryEmpty]);

  const topBar = useMemo(() => (
    <Space placeContent="space-between" gap="1x">
      <Space gap="1x">
        {showEditQueryButton ? editQueryButton : null}
        {!usedCubes.length ? (
          <Title preset="h6">All members</Title>
        ) : (
          <TooltipProvider
            title="Toggle between all members and only those that are used in the query"
            placement="top"
          >
            <Button
              qa="ToggleMembersButton"
              qaVal={viewMode === 'all' ? 'all' : 'used'}
              type={viewMode === 'all' ? 'outline' : 'primary'}
              size="small"
              icon={viewMode === 'all' ? <StarOutlined /> : <StarFilled />}
              onPress={() => setViewMode(viewMode === 'all' ? 'query' : 'all')}
            >
              {viewMode === 'all' ? 'All members' : 'Used only'}
            </Button>
          </TooltipProvider>
        )}
        {isVerifying || isMetaLoading ? <LoadingOutlined /> : null}
      </Space>
      <Space gap=".5x">
        <TooltipProvider title="Reset the query">
          <Button
            qa="ResetQuery"
            aria-label="Reset the query"
            size="small"
            type="secondary"
            theme="danger"
            icon={<ClearIcon />}
            onPress={() => {
              clearQuery();
              setOpenCubes(
                cubesOrViews.length === 1 ? new Set([cubesOrViews[0].name]) : new Set()
              );
              resetScrollAndContentSize();
            }}
          >
            Reset
          </Button>
        </TooltipProvider>
      </Space>
    </Space>
  ),
  [viewMode, isQueryEmpty, isMetaLoading, usedMembers.length, appliedFilterString, isVerifying]);

  const content = (
    <>
      <DialogContainer isOpen={isPasteDialogOpen} onDismiss={() => setIsPasteDialogOpen(false)}>
        <EditQueryDialogForm
          query={query}
          defaultType="json"
          apiVersion={apiVersion}
          onSubmit={onApplyQuery}
        />
      </DialogContainer>

      {!usedCubes.length ? <>{customTypeSwitcher ?? typeSwitcher}</> : topBar}

      {searchInput}

      {organizationSelect}

      {appliedFilterString && !filteredCubes.length ? (
        <Space
          placeContent="space-between"
          placeItems="baseline"
          border="top"
          margin="0 -1x"
          padding="1x 1x 0 1x"
        >
          {!filteredCubes.length ? (
            <Text preset="c2">
              No {selectedType === 'cubes' ? 'cubes' : 'views'} or members found
            </Text>
          ) : null}
        </Space>
      ) : undefined}

      <Panel margin="0 -1x" border="top" flexGrow={1}>
        {cubeList}
      </Panel>
    </>
  );

  return disableSidebarResizing ? (
    <Panel
      key="disabled-siderbar-resizing"
      ref={containerRef}
      isFlex
      qa="QueryBuilderSidePanel"
      flow="column"
      padding="1x 1x 0 1x"
      gap="1x"
      border="1ow right"
      width={width ?? `max ${DEFAULT_SIDEBAR_SIZE}px`}
      innerStyles={{
        overflowX: 'clip',
      }}
    >
      {content}
    </Panel>
  ) : (
    <ResizablePanel
      key="resizable-siderbar"
      ref={containerRef}
      isFlex
      qa="QueryBuilderSidePanel"
      flow="column"
      direction="right"
      size={disableSidebarResizing ? DEFAULT_SIDEBAR_SIZE : sidebarSize}
      isDisabled={disableSidebarResizing}
      minSize={DEFAULT_SIDEBAR_SIZE}
      maxSize="35%"
      padding="1x 1x 0 1x"
      gap="1x"
      innerStyles={{
        overflowX: 'clip',
      }}
      onSizeChange={setSidebarSize}
    >
      {content}
    </ResizablePanel>
  );
}
