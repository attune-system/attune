# Testing Guide: Dashboard & Rules Pages

This guide covers manual testing of the newly implemented dashboard and rules management pages in the Attune Web UI.

---

## Prerequisites

### 1. Backend Services Running

You need the following services running:

```bash
# Terminal 1: PostgreSQL (if not running as service)
docker run -d --name attune-postgres \
  -e POSTGRES_PASSWORD=attune \
  -e POSTGRES_USER=attune \
  -e POSTGRES_DB=attune \
  -p 5432:5432 postgres:14

# Terminal 2: RabbitMQ (if not running as service)
docker run -d --name attune-rabbitmq \
  -p 5672:5672 -p 15672:15672 \
  rabbitmq:4.3.6-management-alpine

# Terminal 3: API Server
cd crates/api
cargo run
# Should start on http://localhost:8080
```

### 2. Test Data

Create test data using the CLI or API:

```bash
# Using the CLI
cd crates/cli

# Create a test pack
cargo run -- pack create \
  --name test-pack \
  --version 1.0.0 \
  --description "Test pack for UI testing"

# Create a test action
cargo run -- action create \
  --pack test-pack \
  --name test-action \
  --entry-point "echo 'Hello World'" \
  --runner-type local.shell.command

# Create a test trigger
cargo run -- trigger create \
  --pack test-pack \
  --name test-trigger \
  --description "Manual test trigger"

# Create a test rule
cargo run -- rule create \
  --pack test-pack \
  --name test-rule \
  --trigger test-trigger \
  --action test-action \
  --description "Test automation rule"

# Execute the action to create execution records
cargo run -- action execute test-pack.test-action
cargo run -- action execute test-pack.test-action
cargo run -- action execute test-pack.test-action
```

### 3. Web UI Running

```bash
# Terminal 4: Web UI Dev Server
cd web
npm install  # First time only
npm run dev
# Should start on http://localhost:5173
```

### 4. Login Credentials

Default test user (if seeded):
- Username: `admin`
- Password: `admin`

Or create a user via API/CLI if needed.

---

## Dashboard Testing

### Test 1: Initial Load

**Objective**: Verify dashboard loads with correct metrics.

**Steps**:
1. Navigate to `http://localhost:5173`
2. Login if not authenticated
3. Should automatically redirect to dashboard

**Expected Results**:
- ✅ Dashboard page loads without errors
- ✅ Four metric cards display at top
- ✅ Each metric shows a number (not "—" or "Loading...")
- ✅ Metric counts match actual data:
  - Total Packs: should show 1+ (your test packs)
  - Active Rules: should show count of enabled rules
  - Running Executions: likely 0 (unless something is running)
  - Total Actions: should show 1+ (your test actions)

### Test 2: Live Connection Indicator

**Objective**: Verify SSE connection status is shown.

**Steps**:
1. On dashboard, look for "Welcome back" message
2. Next to it should be a "Live" indicator

**Expected Results**:
- ✅ Green pulsing dot visible
- ✅ "Live" text displayed in green
- ✅ If API is stopped, indicator should disappear within 30s

### Test 3: Metric Card Navigation

**Objective**: Verify clicking metrics navigates to correct pages.

**Steps**:
1. Click "Total Packs" card → should go to `/packs`
2. Go back, click "Active Rules" card → should go to `/rules`
3. Go back, click "Running Executions" card → should go to `/executions`
4. Go back, click "Total Actions" card → should go to `/actions`

**Expected Results**:
- ✅ Each click navigates to correct page
- ✅ Hover effect shows on cards (shadow increases)
- ✅ Cursor shows pointer on hover

### Test 4: Status Distribution Chart

**Objective**: Verify execution status visualization.

**Steps**:
1. Look at "Execution Status" section (left side, below metrics)
2. Should show status breakdown with progress bars

**Expected Results**:
- ✅ Status categories listed (succeeded, failed, running, etc.)
- ✅ Counts displayed for each status
- ✅ Progress bars show percentage visually
- ✅ Colors match status (green=succeeded, red=failed, blue=running)
- ✅ Success rate displayed at bottom
- ✅ If no executions: "No executions yet" message

### Test 5: Recent Activity Feed

**Objective**: Verify execution activity list.

**Steps**:
1. Look at "Recent Activity" section (right side, 2 columns wide)
2. Should show list of recent executions

**Expected Results**:
- ✅ Up to 20 executions displayed
- ✅ Each shows: pack.action name, status badge, ID, time, elapsed time
- ✅ Clicking an item navigates to execution detail page
- ✅ Hover effect highlights row
- ✅ "View all →" link goes to executions page
- ✅ If no executions: "No recent activity" message

### Test 6: Real-Time Updates

**Objective**: Verify SSE updates dashboard in real-time.

**Steps**:
1. Keep dashboard open in browser
2. In terminal, execute an action:
   ```bash
   cargo run -- action execute test-pack.test-action
   ```
3. Watch the dashboard

**Expected Results**:
- ✅ Recent Activity updates within 1-2 seconds
- ✅ New execution appears at top of list
- ✅ Running Executions count updates if execution is in progress
- ✅ Status distribution updates when execution completes
- ✅ No page reload required
- ✅ "Live" indicator stays green throughout

### Test 7: Quick Actions Section

**Objective**: Verify navigation cards at bottom.

**Steps**:
1. Scroll to bottom of dashboard
2. Should see "Quick Actions" section with 3 cards

**Expected Results**:
- ✅ Three cards: "Manage Packs", "Browse Actions", "Configure Rules"
- ✅ Each has an icon and description
- ✅ Hover effect shows (shadow increases)
- ✅ Clicking navigates to correct page

### Test 8: Responsive Layout

**Objective**: Verify layout adapts to screen size.

**Steps**:
1. Resize browser window from wide to narrow
2. Observe metric cards layout

**Expected Results**:
- ✅ Desktop (>1024px): 4 columns of metrics
- ✅ Tablet (768-1024px): 2 columns of metrics
- ✅ Mobile (<768px): 1 column of metrics
- ✅ Status chart and activity feed stack on mobile
- ✅ No horizontal scrolling at any size

---

## Rules Pages Testing

### Test 9: Rules List - Initial Load

**Objective**: Verify rules list page displays correctly.

**Steps**:
1. Navigate to `/rules` or click "Configure Rules" from dashboard
2. Should see rules list page

**Expected Results**:
- ✅ Page title "Rules" visible
- ✅ Description text visible
- ✅ Filter buttons visible (All Rules, Enabled, Disabled)
- ✅ "Create Rule" button visible (disabled/placeholder for now)
- ✅ Result count shows "Showing X of Y rules"
- ✅ Table with headers: Rule, Pack, Trigger, Action, Status, Actions
- ✅ Test rule visible in table

### Test 10: Rules List - Filtering

**Objective**: Verify filtering works correctly.

**Steps**:
1. On rules list page, note initial count
2. Click "Enabled" filter button
3. Note filtered count
4. Click "Disabled" filter button
5. Click "All Rules" button

**Expected Results**:
- ✅ "Enabled" shows only enabled rules
- ✅ "Disabled" shows only disabled rules
- ✅ "All Rules" shows all rules
- ✅ Active filter button highlighted in blue
- ✅ Inactive buttons are gray
- ✅ Count updates correctly with each filter

### Test 11: Rules List - Toggle Enable/Disable

**Objective**: Verify inline status toggle.

**Steps**:
1. On rules list, find a rule with "Enabled" status
2. Click the green "Enabled" badge
3. Wait for update
4. Observe status change

**Expected Results**:
- ✅ Badge shows loading state briefly
- ✅ Status changes to "Disabled" (gray badge)
- ✅ Clicking again toggles back to "Enabled"
- ✅ No page reload
- ✅ If "Enabled" filter active, rule disappears from list after disable

### Test 12: Rules List - Delete Rule

**Objective**: Verify rule deletion.

**Steps**:
1. On rules list, click "Delete" button for a test rule
2. Confirmation dialog appears
3. Click "Cancel" first
4. Click "Delete" again
5. Click "OK" to confirm

**Expected Results**:
- ✅ Confirmation dialog shows rule name
- ✅ Cancel does nothing
- ✅ OK removes rule from list
- ✅ Count updates
- ✅ No page reload

### Test 13: Rules List - Pagination

**Objective**: Verify pagination controls (if >20 rules).

**Steps**:
1. Create 25+ rules (if needed)
2. On rules list, observe pagination controls at bottom
3. Click "Next" button
4. Click "Previous" button

**Expected Results**:
- ✅ Pagination only shows if >20 rules
- ✅ "Page X of Y" displayed
- ✅ "Next" disabled on last page
- ✅ "Previous" disabled on first page
- ✅ Navigation works correctly

### Test 14: Rule Detail - Basic Information

**Objective**: Verify rule detail page displays all info.

**Steps**:
1. From rules list, click a rule name
2. Should navigate to `/rules/:id`

**Expected Results**:
- ✅ "← Back to Rules" link at top
- ✅ Rule name as page title
- ✅ Status badge (Enabled/Disabled) next to title
- ✅ Description visible (if set)
- ✅ Metadata: ID, created date, updated date
- ✅ Enable/Disable button at top right
- ✅ Delete button at top right

### Test 15: Rule Detail - Overview Card

**Objective**: Verify overview section content.

**Steps**:
1. On rule detail page, find "Overview" card (left side)
2. Check displayed information

**Expected Results**:
- ✅ Pack name displayed as clickable link
- ✅ Trigger name displayed
- ✅ Action name displayed as clickable link
- ✅ Clicking pack link goes to `/packs/:name`
- ✅ Clicking action link goes to `/actions/:id`

### Test 16: Rule Detail - Criteria Display

**Objective**: Verify criteria JSON display (if rule has criteria).

**Steps**:
1. On rule detail, look for "Match Criteria" card
2. Should show JSON formatted criteria

**Expected Results**:
- ✅ Card only appears if criteria exists
- ✅ JSON is formatted with indentation
- ✅ Displayed in monospace font
- ✅ Gray background for readability
- ✅ Scrollable if content is long

### Test 17: Rule Detail - Action Parameters

**Objective**: Verify action parameters display.

**Steps**:
1. On rule detail, look for "Action Parameters" card
2. Should show JSON formatted parameters

**Expected Results**:
- ✅ Card only appears if parameters exist
- ✅ JSON is formatted with indentation
- ✅ Displayed in monospace font
- ✅ Gray background for readability
- ✅ Scrollable if content is long

### Test 18: Rule Detail - Quick Links Sidebar

**Objective**: Verify quick links functionality.

**Steps**:
1. On rule detail, find "Quick Links" card (right sidebar)
2. Try clicking each link

**Expected Results**:
- ✅ "View Pack" link works
- ✅ "View Action" link works
- ✅ "View Trigger" link works (may 404 if triggers page not implemented)
- ✅ "View Enforcements" link works (may 404 if enforcements page not implemented)

### Test 19: Rule Detail - Metadata Sidebar

**Objective**: Verify metadata display.

**Steps**:
1. On rule detail, find "Metadata" card (right sidebar)
2. Check all fields

**Expected Results**:
- ✅ Rule ID in monospace font
- ✅ Pack ID in monospace font
- ✅ Trigger ID in monospace font
- ✅ Action ID in monospace font
- ✅ Created timestamp in readable format
- ✅ Last Updated timestamp in readable format

### Test 20: Rule Detail - Status Card

**Objective**: Verify status display and warnings.

**Steps**:
1. On rule detail, find "Status" card (right sidebar)
2. If rule is disabled, should show warning

**Expected Results**:
- ✅ Status badge shows "Active" or "Inactive"
- ✅ Color matches enabled state (green/gray)
- ✅ If disabled: warning message displayed
- ✅ Warning text explains rule won't trigger

### Test 21: Rule Detail - Enable/Disable Toggle

**Objective**: Verify status toggle on detail page.

**Steps**:
1. On rule detail page, click Enable/Disable button
2. Watch for status update
3. Toggle back

**Expected Results**:
- ✅ Button shows loading state ("Processing...")
- ✅ Status badge updates after success
- ✅ Button text changes (Enable ↔ Disable)
- ✅ Button color changes (green ↔ gray)
- ✅ Status card updates
- ✅ No page reload

### Test 22: Rule Detail - Delete Rule

**Objective**: Verify rule deletion from detail page.

**Steps**:
1. On rule detail page, click "Delete" button
2. Confirmation dialog appears
3. Click "OK"

**Expected Results**:
- ✅ Confirmation dialog shows rule name
- ✅ After confirmation, redirects to `/rules` list
- ✅ Rule no longer in list
- ✅ No errors

---

## Error Handling Testing

### Test 23: Network Error Handling

**Objective**: Verify graceful handling of network errors.

**Steps**:
1. Stop the API server
2. Refresh dashboard or rules page
3. Wait for timeout

**Expected Results**:
- ✅ Loading spinner shows while attempting
- ✅ Error message displayed after timeout
- ✅ "Live" indicator disappears
- ✅ Page doesn't crash
- ✅ Can navigate to other pages

### Test 24: Invalid Rule ID

**Objective**: Verify handling of non-existent rule.

**Steps**:
1. Navigate to `/rules/99999` (non-existent ID)

**Expected Results**:
- ✅ Error message displayed
- ✅ "Rule not found" or similar message
- ✅ "Back to Rules" link provided
- ✅ No page crash

### Test 25: SSE Reconnection

**Objective**: Verify SSE reconnects after interruption.

**Steps**:
1. Open dashboard with "Live" indicator active
2. Stop API server
3. Wait 30 seconds (indicator should disappear)
4. Restart API server
5. Wait up to 30 seconds

**Expected Results**:
- ✅ "Live" indicator disappears when connection lost
- ✅ Dashboard still usable (cached data)
- ✅ "Live" indicator reappears after reconnection
- ✅ Updates resume automatically

---

## Performance Testing

### Test 26: Dashboard Load Time

**Objective**: Verify dashboard loads quickly.

**Steps**:
1. Open browser DevTools → Network tab
2. Clear cache and reload dashboard
3. Observe load time

**Expected Results**:
- ✅ Initial load < 2 seconds (with warm backend)
- ✅ Metrics appear < 3 seconds
- ✅ No excessive API calls (should be ~5 requests)

### Test 27: Large Rules List

**Objective**: Verify performance with many rules.

**Steps**:
1. Create 100+ rules (if feasible)
2. Navigate to rules list page
3. Scroll through list

**Expected Results**:
- ✅ Page loads in reasonable time (< 3s)
- ✅ Only 20 items per page (pagination working)
- ✅ Smooth scrolling
- ✅ No lag when changing pages

---

## Cross-Browser Testing

### Test 28: Browser Compatibility

**Objective**: Verify works in major browsers.

**Browsers to test**: Chrome, Firefox, Safari, Edge

**Steps**:
1. Open dashboard in each browser
2. Test basic navigation
3. Test real-time updates

**Expected Results**:
- ✅ Layout looks correct in all browsers
- ✅ All functionality works
- ✅ SSE connection works (all support EventSource)
- ✅ No console errors

---

## Accessibility Testing

### Test 29: Keyboard Navigation

**Objective**: Verify keyboard accessibility.

**Steps**:
1. Navigate dashboard using only Tab key
2. Press Enter on focused elements

**Expected Results**:
- ✅ All interactive elements focusable
- ✅ Focus indicator visible
- ✅ Logical tab order
- ✅ Enter key activates buttons/links

### Test 30: Screen Reader Testing

**Objective**: Verify screen reader compatibility (basic).

**Steps**:
1. Use browser's reader mode or screen reader
2. Navigate dashboard and rules pages

**Expected Results**:
- ✅ Headings properly announced
- ✅ Button labels descriptive
- ✅ Link text meaningful
- ✅ Form controls labeled

---

## Reporting Issues

If you find any issues during testing:

1. **Check console** (F12 → Console tab) for errors
2. **Note exact steps** to reproduce
3. **Screenshot** if visual issue
4. **Browser/OS** information
5. **Create issue** in project tracker or document in `work-summary/PROBLEMS.md`

---

## Success Criteria

All tests passing means:
- ✅ Dashboard displays live metrics correctly
- ✅ Real-time updates work via SSE
- ✅ Rules CRUD operations fully functional
- ✅ Navigation flows work seamlessly
- ✅ Error handling is graceful
- ✅ Performance is acceptable
- ✅ Cross-browser compatible
- ✅ Accessible to keyboard users

---

## Next Steps After Testing

Once all tests pass:
1. Document any bugs found in PROBLEMS.md
2. Fix critical issues
3. Consider visual enhancements (charts library, animations)
4. Move on to Events/Triggers/Sensors pages
5. Implement create/edit forms for packs, actions, rules
