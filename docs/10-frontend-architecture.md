# 前端架构与 Workflow 画布

## 1. 技术栈

- React
- TypeScript
- Vite
- Tailwind CSS
- Radix UI
- Class Variance Authority
- React Router
- TanStack Query
- TanStack Table
- Zustand
- React Hook Form
- Zod
- i18next
- Lucide React
- React Flow

Tailwind 是唯一的视觉样式体系，不再引入 Ant Design 或另一套完整组件库。Radix UI 只提供无障碍行为和交互原语，最终视觉由 Agentx Token 和 Tailwind Class 决定。

plan6 增加受审核画布插件UI。`plugin-ui-loader.ts`按不可变摘要动态导入编译后的React ESM并引用计数加载CSS；宿主只暴露同一个React运行时和公共`Field/Input/Button/Select/SmartInput`。配置Panel、Canvas内容、Result和Trace renderer都由Error Boundary隔离，卸载最后一个挂载点时清理样式。`node-panel-registry.tsx`是内置与插件面板的统一入口，Palette仍只展示启用版本；打开引用停用版本的Draft时按精确`nodeType/typeVersion`补取冻结Manifest，不把它重新加入Palette。

## 2. 企业工作台

正式布局使用企业工作台方案：

    EnterpriseLayout
    ├── TenantSwitcher
    ├── Sidebar
    │   ├── 工作空间
    │   ├── 运行与质量
    │   ├── 资源
    │   └── 组织
    ├── Header
    │   ├── Breadcrumb
    │   ├── GlobalSearch
    │   ├── ThemeSwitch
    │   ├── Notification
    │   └── UserMenu
    └── PageContent

固定规格：

- Sidebar 展开 248px
- Sidebar 折叠 72px
- Header 64px
- 页面间距 24px 或 32px
- 基础控件高度 36px
- 基础圆角 8px
- 页面容器最大宽度 1480px

## 3. 设计令牌

颜色使用语义名称：

- background
- surface
- sidebar
- foreground
- muted
- muted-foreground
- border
- primary
- success
- warning
- danger
- canvas

业务代码不得用具体灰度或品牌色替代语义 Token。浅色和深色通过同一组语义 Token 切换。

主题偏好包含 system、light 和 dark。浏览器保存用户选择，未设置时跟随操作系统；初始化脚本必须在 React 挂载前解析主题，避免浅深色闪烁。

字体使用本地系统字体栈，不依赖外部字体 CDN。英文优先 Segoe UI，中文依次使用 PingFang SC、Microsoft YaHei 和 Noto Sans SC。

## 4. UI 组件分层

shared/ui 保存 Button、Input、Select、Dialog、Dropdown、Tooltip、Table、Form、Badge、Tabs、Drawer 和 Toast 等基础组件。

shared/components 保存 PageContainer、PageHeader、FilterBar、DataTable、MetricCard、StatusBadge、EmptyState 和 ErrorState 等页面组合组件。

Feature 页面优先组合共享组件，不重复定义页面标题、表格密度、状态颜色和空状态。

`src/web/src/docs` 集中保存面向最终用户的内嵌接入文档、双语内容、动态代码示例和文档展示组件；Feature 页面只传入当前实体、公开 Endpoint 与已发布 Schema，不在业务页面复制长篇说明。Application 的 API Key 与 Webhook 文档以当前激活 Deployment 为数据源，在尚未创建 API Key 或 Webhook 时也必须能直接查看输入/输出 Schema；未创建 Webhook 时只用占位符表示尚未生成的专属 URL 和 Secret。接口参考按 Endpoint 展示请求字段、响应字段和 Curl、Java、Go、Node.js、Python 示例。外部 API 的方法、路径、Header 和 Schema 仍以版本化 OpenAPI 为契约真相，内嵌文档负责快速开始、安全说明和错误排查，不替代 OpenAPI。

## 5. Feature 边界

    features/
    ├── workflows/
    ├── workflow-designer/
    ├── applications/
    ├── executions/
    ├── approvals/
    ├── datasets/
    ├── evaluations/
    ├── models/
    ├── mcp/
    ├── skills/
    ├── knowledge/
    ├── memory/
    ├── resource-grants/
    └── organization/

Feature 不得引用其他 Feature 的内部文件。共享逻辑进入 shared，稳定的业务协议通过 Feature 公共入口暴露。

Mock 数据属于对应 Feature，只用于基础界面展示和本地筛选，不能进入 shared/ui，也不能模拟服务端权威状态或完整 CRUD。

## 6. 国际化

- 首期支持 zh-CN 和 en-US，默认回退到 zh-CN。
- 已保存语言优先于浏览器语言，语言偏好保存到 agentx.locale。
- 菜单、页面、状态、提示和演示数据全部使用翻译资源，不在页面中维护成对文案。
- 语言切换后同步更新 html lang。
- 菜单配置只保存翻译 Key，Sidebar、面包屑和全局搜索共享同一份配置。

## 7. React Flow

Workflow 画布使用 React Flow，不自行实现拖动、缩放、框选、Handle、Edge、MiniMap 和 Viewport。

Agentx 自己实现：

- 节点业务组件
- main、error 和 AI 资源连接
- 节点配置面板
- 连线合法性校验
- Workflow Draft 序列化
- 自动保存
- Undo 和 Redo
- 运行状态高亮
- Pin Data
- Checkpoint 和部分执行入口
- Node Catalog/Manifest 表单渲染
- Draft Revision 调试快照入口

推荐目录：

    workflow-designer/
    ├── canvas/
    ├── nodes/
    ├── edges/
    ├── panels/
    ├── forms/
    ├── api/
    ├── store/
    ├── model/
    └── utils/

以下Studio规格是[plan5](plan5/README.md)冻结并实现的桌面端边界；业务、视觉和性能验收结果以实施计划记录的命令与证据为准。

Studio 采用 Dify 式节点配置流程与 n8n 式紧凑桌面画布：左侧为 264px 常驻且可收起的节点栏，中央为无限 Canvas，单击节点后打开 384px 全高 Node Details，底部为可折叠的全局 Execution Rail。可创建节点、端口、字段和能力来自 Node Manifest/有效契约；前端仅为冻结的13类节点维护专用业务布局，并以静态门禁和Registry对账，不能另写一份长期漂移的节点目录。Feature 内任何文件不得超过 2000 行。

画布视觉为 Dify 式 240px 横向白卡（plan5 重设计）：头部为 24px 彩色图标方块（六组分类色：起始 `#155EEF`、AI `#6366F1`、逻辑 `#06B6D4`、转换 `#3B82F6`、集成 `#8B5CF6`、输出 `#F59E0B`）+ 标题 + 运行状态；节点体承载 0–3 行配置摘要，`if`/`approval` 按参数渲染分支行且每行右侧独立连接点（`case:{id}` / `decision:{id}`），`merge` 左侧堆叠多个输入行。连接点为左入右出的 3×10px 竖条，Error 端口红色常驻。角色枚举已随 `error_handler` 删除收敛。AI 附件不再是画布节点：工具/技能/知识/记忆收进 Agent 节点的内嵌槽位（节点体附件徽标行 + Inspector 槽位管理，`resourceReferences[].bindingRole`），`loop_over_items` 使用 React Flow 父子容器（`parentId` + `extent` + NodeResizer + 迭代开始 chip）；chip与派生入口线仅属于视图，不进入Definition。便签和分组仍是仅编辑态派生节点；节点面板常驻左侧（可收起），保留端口兼容过滤、多输入选择和拖拽添加。缩放降级（full/compact/minimal）随新卡片结构重写。

连接交互使用独立状态机维护source hover、connecting、compatible、incompatible、occupied、committed和cancelled；合法性通过增量GraphIndex校验方向、类型、基数、自连接、重复边和动态Handle。非variadic输入已占用时以一个历史事务替换旧边；存在多个兼容输入时必须显式选择端口。普通执行边使用实线箭头，Error边使用危险色虚线箭头；AI资源直接写入`resourceReferences`，不再绘制Binding边。反向主连接使用固定底部间距、水平偏移和圆角路由。Edge Hover/选中工具栏只提供插入和删除，重新连接通过React Flow边端点拖拽完成。

节点栏、圆形 `+`、空画布入口、端口快速添加和边上插入共用同一搜索、分类与合法端口过滤；节点栏收起时快捷入口可临时展开同一创建面板。正常缩放下，未占用的执行输出默认展示 `+`；已占用的非 `variadic` 端口不再展示，低于0.65缩放时统一隐藏。便签和视觉分组命令与运行节点分区显示。单击执行节点立即打开384px Details，单击空画布清除选择并关闭；关闭按钮和Esc只关闭Details并保留选择。Details头部保留运行当前节点，参数/Input/Output/Trace与Pin/Mock形成同一调试闭环。

Manifest `localizations` 是节点名称、说明、搜索关键词、端口和 Binding Slot 展示名的唯一业务本地化来源。显示按当前语言、`en-US`、基础字段依次回退，协议 ID 不翻译。Start 是不可删除的固定输入边界，配置Inputs/Contexts；Exit是真实终止节点，配置共享输出契约与本出口取值。二者使用专用组件和保护规则，不作为普通Catalog节点处理。

画布资源字段统一使用基于 Radix Popover 的 `ResourcePicker`。资源选项 Query Key 包含 Workflow、Resource Type 和 Operation，前端遍历后端分页得到全部当前可见资源，再提供即时搜索；同一类型的不同操作不得复用错误的授权状态。资源行的选择按钮和行尾动作按钮是两个独立交互目标。未授权行不可选择，但可以直接“授权”或“申请”；Pending 提供申请详情入口，Rejected 提供重新申请。存在 Pending 项时每 5 秒轮询，窗口重新聚焦时强制刷新。直接授权或审批完成只失效并刷新 TanStack Query，不调用 `onChange`，用户必须重新打开 Picker 手动选择。已选资源撤权、禁用或失去可见性时保留原引用，显示脱敏危险状态，并由统一资源校验阻止保存、创建版本和运行。

## 8. 画布状态转换

React Flow 的 Node 和 Edge 结构只属于前端编辑状态，不能直接成为后端运行协议。Serializer 同时产出运行 Definition 和纯 UI Editor Document，Debug Overlay/运行高亮走独立模型。

    React Flow State
          ├── serialize definition ──> Workflow Definition 8.0 ──> IR
          └── serialize editor ──────> Editor Document

    Pin / Mock / Runtime Highlight ──> Debug Overlay / Execution State

转换层负责：

- 删除纯 UI 字段
- 固化 Node Type Version
- 将 Handle 映射为端口
- 将 Edge 映射为连接类型和稳定 Connection Order
- 校验资源引用和授权状态
- 将位置、视口、注释和分组只写入 Editor Document

服务端Draft Revision原子保存Definition和Editor Document，并在写入前执行轻量引用可达性校验；非法引用返回字段级错误且不增加revision。TanStack Query保存服务端权威数据；Zustand Editor Store按Document、Interaction、History三个逻辑slice维护规范化编辑状态；History以实体ID保存节点、边、便签和分组的前后Patch，不保存视口、选择、Hover、临时连线或运行高亮；Runtime Overlay Store按executionId隔离运行状态和结果。四者不得互相复制成为第二权威。

所有声明`x-agentx-binding`的Reference、Value、Template、Structured、JSON和Mapper叶子共用Reference Picker。文本类字段使用Lexical Token Editor，将普通文本与变量Chip作为独立节点混排，支持光标、选区、Backspace/Delete、Undo/Redo、中英文IME，以及携带Agentx自定义MIME的复制粘贴；UI不展示协议字符串。普通字段不再提供表达式类型或递归Visual Builder，IF/List使用专用ConditionSpec，复杂转换进入Code。

引用树只读取Start Inputs、声明Context和当前节点经execution边可达前驱的Effective Output Contract；binding/resource关系不解锁输出引用。没有明确目标节点时输出目录为空，Exit main/error分别使用自己的虚拟目标。选择器以Node ID持久化，以节点名称、端口和字段路径显示；节点重命名不修改Selector。断线旧引用保留并显示错误，不自动删除。运行历史只能提供非权威样例值。敏感Context不允许插入普通字段或Preview。

## 9. 画布扩展

- ELK.js 用于自动布局。
- Zustand 保存编辑器状态。
- History slice 以手势事务管理 Undo 和 Redo；拖动开始记录参与实体旧位置、结束只提交这些实体的一个 Patch 命令。
- Monaco Editor 只用于 Code；Prompt/Template 使用 Lexical Token Editor，Expression 使用 Visual Builder，JSON/Mapper 使用结构化叶子绑定控件。
- 大型Workflow在150节点启用可见元素渲染、300节点隐藏MiniMap；缩放低于0.65隐藏标签，低于0.35使用简化内容。`IncrementalGraphIndex`稳定复用`nodeById`、`portByHandle`、源/目标端口边索引和资源引用摘要Map，只更新结构、端口、资源引用或边的变化项；位置帧不重建连接索引。Group、Node和Edge视图索引保持无关对象引用稳定，节点运行态只替换自身与关联边。
- 自动保存使用 Server Revision；Undo/Redo 只改变本地 Editor State，不能回退服务端 Revision。
- Execution Event 使用 Cursor 重连，断线后通过 Execution Query 校准；非终态运行更新最多每 100ms 合并一次，终态立即提交；完整 Trace 和大型输出不进入 Zustand。

Workflow Studio 底部 Trace、独立执行详情和 Node Inspector 复用 `features/traces` 中的 `TraceWorkspace/TraceNodeDetails`。Workspace 默认进入节点视图：顶部固定显示最终输出、状态、耗时、Token 和成本，主体按 Start Boundary、每个 Node Execution、End Boundary 排列；Node 卡片以 Runtime MySQL 的输入输出为权威，按 Port/Item 解包并优先展示稳定语义字段，内部 Span 默认折叠且只在节点展开时携带 `nodeExecutionId` 加载。Trace 延迟或不可用必须显示明确诊断状态，不能用空输入替代权威数据。三个入口必须向共享内容组件传递同一个按 Execution 作用域授权的 Artifact 下载动作；超过 Trace 内联预算的内容只展示脱敏 Preview 和 Artifact 入口，浏览器不得绕过 `/executions/{executionId}/artifacts/{artifactId}` 直接访问对象存储。

高级调用瀑布复用分页 Query Hook、`TraceWaterfall` 和 `TraceDetail`。Span 数据只保存在 TanStack Query Cache，不复制到 Zustand。瀑布保留筛选匹配项的祖先上下文，支持层级折叠、类型/异常筛选、搜索、50%–200% 时间轴缩放、键盘 treegrid 操作和继续加载；默认选择首个失败 Span，其次运行中 Span，最后选择 Execution 根 Span。详情按 Span 类型与 `contentKind` 动态生成页签，纯生命周期 Span 不显示误导性的空输入/输出。运行中宽度按当前时间刷新，终态只使用服务端耗时。

桌面端Node Inspector固定384px；Trace高级详情列在执行轨道内按可用宽度布局，低于1120px后移到瀑布下方。Studio 首次进入 Trace 页签时，执行轨道扩展到最多 560px（同时受视口 80% 上限约束），但不缩小用户手动设置的更大高度；用户继续向上拖动时最高可占视口的 4/5。轨道自身裁剪溢出，内部节点视图、瀑布和详情各自滚动，避免详情遮挡画布或相邻面板。选择 Node 或子 Span 时联动画布节点。Node Inspector 直接复用单节点语义详情，不再自行搜索 Span 后套通用详情。独立执行详情使用 Trace 和 Recovery 两个主视图并默认显示 Trace；Recovery 只保留 Checkpoint、Wait、Approval、Side Effect、事件与 Fork，删除重复的节点 Outline 和输入输出面板。

## 10. 路由

一级路由与企业工作台菜单保持一致：

- /
- /workflows
- /applications
- /playground
- /executions
- /approvals
- /approvals/resource-grants/:id
- /datasets
- /evaluations
- /models
- /mcp
- /skills
- /knowledge
- /memory
- /resource-grants
- /organization
- /roles
- /workflows/:workflowId/editor

Playground 调用正式 Application API，不建立单独的执行实现。页面用 `mode=parameters|conversation`、`applicationId` 和 `sessionId` 恢复刷新前状态，运行与消息历史全部来自服务端。

参数测试采用“无 Session 运行历史 / Schema 表单 / 结果详情”三栏布局。共享 `SchemaForm` 负责 string、multiline、enum、number、integer、boolean、object、array、Artifact、Artifact Array、递归默认值、恢复默认和 JSON 模式，Workflow Studio 的 Run Parameters 与 Playground 复用该组件。选择历史运行时，表单以当前 Deployment Schema 默认值为基础，只回填 Execution 输入中名称与类型仍兼容的字段，忽略旧版本已删除或改型的参数。Artifact 先上传到 `/gateway/v1/artifacts`，随后用 Artifact Reference 调用正式 Invocation API；结果面板展示输出、错误、耗时、Token、成本、文件和 Execution/Trace 入口。

对话测试采用“会话历史 / 消息历史与固定 Composer”两栏布局，工作区填满页面剩余高度，各栏内部独立滚动。共享映射配置放在 Radix Dialog 中，由带 Tooltip 和可访问名称的齿轮图标打开，只列出类型兼容的顶层字段；没有活动映射时发送按钮禁用，附件按钮在未映射文件输入时禁用。Composer 使用多行矩形输入框，附件 `+` 在左下角，发送图标在右下角，Enter 发送且 Shift+Enter 换行。`application:manage` 可以保存或清除映射，只有 `application:invoke` 的用户可以使用已发布映射。保存后轮询 `publishing/active/failed`，发布期间保留服务端草稿并暂停发送；用户与 Assistant 附件都渲染文件卡片，Assistant 消息提供 Execution/Trace 入口。Session 列表使用首条用户问题作为标题，单行省略并在悬停时展示完整标题。

首期企业工作台只验收 1280px 及以上桌面布局。Sidebar 折叠状态保存到 agentx.sidebar.collapsed；不实现移动端 Drawer。普通一级列表页继续使用本地搜索、状态筛选、8 条分页和无结果状态；执行记录页是受控服务端表格的明确例外，不能加载固定 100 条后本地筛选。

执行记录页的搜索、状态、时间和更多筛选写入 URL，Cursor 只保存在当前页面内存中。TanStack Query Key 使用规范化筛选条件和当前 Cursor；文本输入 300ms 防抖，任何筛选变化清空 Cursor 栈。公共查询统一使用 `limit`，页面固定为 8，通过 Cursor 栈实现上一页/下一页；Cursor 过期时回到第一页并明确提示结果已刷新。应用、Workflow、MCP Tool、用户和部门选项使用共享的可搜索多选 Popover 按需加载，其中部门使用 `/departments/search`，避免改变组织树接口语义；已选条件显示可移除标签与“清除全部”。

执行表格展示当前 Control 名称补充与 Runtime 审计快照的不同语义：应用和 Workflow 名称是 BFF 对当页稳定 ID 批量解析的当前名称；发起用户、发起部门和触发名称来自执行时快照。未知触发枚举保留协议原值用于诊断，已知值必须使用 `executions.triggerTypes` 的中英文映射。

### 10.1 部署后的页面资源加载

HTML 入口及 `runtime-config.js` 使用 `Cache-Control: no-store`；带内容摘要的 `/assets/` 文件允许长期不可变缓存，缺失资源必须返回不缓存的 404，不能返回 SPA 首页。SPA 业务路由仍由 `index.html` 承接。

入口监听 Vite 官方 `vite:preloadError` 事件。每个入口构建在同一个浏览器标签页内最多自动刷新一次，以获取新部署的资源；刷新后仍失败或标签页存储不可用时，由全局路由错误页提供明确提示、手动刷新与返回首页，不显示框架默认开发者错误页。此行为不保留旧版本资源或旧路径兼容入口。实现依据见 [Vite 加载错误处理](https://vite.dev/guide/build#load-error-handling) 与 [NGINX 响应头文档](https://nginx.org/en/docs/http/ngx_http_headers_module.html)。

Kubernetes 桌面浏览器验收覆盖创建工作流后进入详情和画布、单次缺失分块恢复且不重复创建、持续缺失时不循环刷新，以及显式刷新恢复。

## 11. M2.1 资源界面

- Model 列表只提供“新建模型”，在同一个 Dialog 中按连接、模型能力、所属部门和默认参数的顺序完成配置，不要求预先创建连接。Model 详情使用相同字段集合统一编辑；连接或模型参数变化都创建不可变 Deployment Revision，并展示 Revision 历史。
- Credential 详情允许修改名称和状态，但绝不把 Secret 放入表单默认值或 Query Cache。
- MCP 列表和详情替代旧 Tool 页面，Tool Schema 使用只读 JSON Schema 字段树展示名称、类型、必填、说明、约束及嵌套关系，策略和受控调试使用独立 Dialog。
- Skill 详情采用文件树、编辑/预览区和元数据区三栏布局；根 `SKILL.md` 在正文上方使用多行文本框独立编辑必填描述并隐藏 YAML frontmatter 细节。Markdown 正文使用 MIT 许可的 MDXEditor/Lexical 富文本层并继续保存标准 Markdown，工作区文件引用从富文本工具栏选择并插入当前光标位置；上传和 ZIP 导入继续走统一 API Client。
- 资源详情页不嵌入授权面板；`/resource-grants` 使用资源类型 Tab 分栏聚合 Credential、Model、MCP Server、MCP Tool、Skill、Knowledge 和 Memory，每栏列出对应资源并统一处理 Department 与 Workflow Service Identity Grant。Workflow Studio 允许对当前工作流的完整资源依赖包发起直接授权或审批申请，这是该统一授权控制面的任务内快捷入口。
- Model 列表展示与当前 Alias/Deployment 配置绑定的 `untested/healthy/unhealthy` 连接状态，连接测试从 Alias 详情页手动触发。
- Dialog 默认按内容完整展开并垂直居中；字段超过六项的通用表单自动使用双列宽布局，只有内容真实超过 `100dvh - 32px` 时才允许外层滚动。
- Workflow 最小画布通过 Serializer 保存 MCP Tool 资源引用，运行按钮在 Runtime 接入前保持禁用。

## 12. 前置条件交互

- 权限允许但缺少业务依赖时，操作按钮保持可点击，并通过共享 `PrerequisiteAction` 展示缺少项、已完成项和下一步入口。
- 只有请求加载中、正在提交或防止重复操作时可以直接禁用按钮；禁止使用无说明的灰色按钮表达“尚未创建依赖数据”。
- 缺少依赖且用户有创建权限时提供准确业务跳转；无创建权限时提示联系管理员，不能把“无权限”和“无数据”混为一谈。
- 依赖查询失败必须显示错误与重试，不能按空数组处理；Select 无可用选项时使用相同依赖说明。
- Application、Deployment、Evaluation、Workflow 发布、Dataset 发布、Skill 启用和 Model 编辑统一遵守该规则。
- 跳转创建依赖后应保留来源页面；返回时重新查询依赖并恢复原操作上下文。

## 13. 界面 E2E 门禁

业务 E2E 位于 apps/e2e，使用 1440×900 Chromium 和临时 Kubernetes Namespace。业务数据必须通过可见按钮和表单创建；API 与数据库只能准备环境、清理现场和校验证据。新增或修改业务按钮时必须补充真实点击测试，危险操作同时覆盖取消和确认，中英文与浅深主题纳入阶段验收。

## 14. 业务域国际化

翻译资源按 `common`、`navigation`、`auth`、`workflows`、`studio`、`applications`、`executions`、`approvals`、`notifications`、`datasets`、`evaluations`、`runtime`、`credentials`、`models`、`mcp`、`skills`、`knowledge`、`memory`、`sandbox`、`resourceGrants`、`organization`、`roles` 和 `errors` 业务域拆分。禁止新增 `m2/m3/m4/m5/m7` 或跨业务 `pages` 顶层命名空间。

`common.*` 只保存保存、取消、删除、状态、分页、确认等跨业务含义完全一致的文案。别名、连接、版本发布、轮换等词进入所属业务域；Model 使用 `models.fields.modelName` 和 `models.fields.upstreamModelId`，Skill 使用 `skills.fields.name/alias/description`，避免同名参数跨模块串词。中英文资源必须保持相同键结构，语言探测、`agentx.locale`、`html lang` 和稳定错误码翻译保持不变。

中文界面采用以下业务术语：Workflow 为“工作流”、Skill 为“技能”、Memory 为“记忆”、Agent 为“智能体”、Execution 为“执行”、Dataset 为“测试集”、Case 为“用例”、Deployment 为“部署”、Environment 为“环境”、Checkpoint 为“检查点”、Service Identity 为“运行身份”。Workflow Studio 使用“工作流设计器”，Playground 使用“调试台”。组合术语统一使用“工作流版本”“技能版本”“记忆命名空间”“工作流执行”“节点执行”“派生执行”“测试集版本”“测试用例”和“工作流运行身份”。

`Revision` 与不可变 `Version` 必须按语义区分：Draft Revision 为“草稿修订号”，Workspace Revision 为“工作区修订号”，Model Deployment Revision 为“部署配置版本”，其他计数型 Revision 为“修订号”；不可变发布物才称为“版本”。`Trace`、`Runtime`、`Worker`、`Artifact`、`Definition`、`Diff`、`Schema`、`Endpoint`、`Runner`、`Token` 保留英文，Agentx、MCP、API、HTTP、JSON、MIME、URI、UUID、OpenSandbox、Mem0、SKILL.md 等标准名或产品名同样不翻译。技术词与中文组合时采用自然语序，例如“智能体运行记录”“Runtime 调用”“本次执行的 Artifact”“评测 Trace”。

稳定接口枚举必须在所属业务域建立翻译映射，普通业务区域不得裸显 `active`、`manual_upgrade`、`resource_missing_or_disabled` 等协议值；未知枚举统一显示“未知（原始值）”并保留诊断信息。协议事件名、字段名、ID、Hash 和 JSON 只允许在明确的技术调试区域原样显示。日期、时间和数字统一通过共享 locale 格式函数按 `i18n.resolvedLanguage` 渲染，非法日期显示 `—`。用户录入的资源名、节点自定义名、代码和协议值不自动翻译；Manifest 文案按当前语言、`en-US`、基础字段顺序回退。

## 15. 列表安全删除

所有支持物理删除的实体只在列表或子对象列表使用共享 `EntityDeleteButton` 和 `EntityDeleteDialog`。列表行操作统一显示“删除”文字按钮，不使用仅图标按钮；按钮保留 ARIA 标签和危险色，禁用时通过 Tooltip 说明原因。删除确认不使用 `window.confirm`，父实体详情页头不增加删除入口。无 `*:delete` 权限时隐藏；根部门、内置角色和内置环境显示禁用按钮及原因。

Dialog 打开后分页加载删除影响。无引用时显示不可撤销确认；有引用时按模块展示来源名称、关系、节点位置和跳转入口并禁用确认。删除期间禁止关闭和重复提交；成功后刷新对应 Query。若 DELETE 因竞态返回 409，直接以错误 `details` 中最新的 `DeletionImpactResponse` 刷新当前 Dialog。

父实体详情页不放置父实体删除入口。Webhook 和 Schedule 当前没有独立列表路由，因此其删除按钮仅作为 Application 详情页内的子对象列表操作；按钮仍使用相同权限、预检和事务复检契约，Application 本身只能从 `/applications` 列表删除。

## 16. 表单字段错误

共享表单和非共享业务表单统一解析 `ApiClientError.detail.fieldErrors`。字段错误显示在对应控件下，设置 `aria-invalid`/`aria-describedby`，失败后聚焦首个错误字段；编辑时只清除当前字段错误。已知错误码使用本地化文案，未知安全 4xx 使用服务端公开 message，5xx 使用通用文案并显示 requestId。
