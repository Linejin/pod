"""D4 proposal tokens for the three directions (A quiet, B layered, C mission control).

Single source for the mockups (build.py), tokens.json and the contrast audit
(check_contrast.py). Same WCAG 2.x formula as tools/visual/contrast.py.
Colors are opaque sRGB hex. Names map to pod-ui tokens in directions.md §6.
"""

# Surface ladder: app (title bar, status bar, sidebar) < panel (terminal, review)
# < raised (menus, cards) < overlay (dialogs). hover/active/selected sit on any.
A = {
    'dark': dict(
        app='#16181C', sidebar='#16181C', panel='#1B1D22', raised='#22252B', overlay='#272A31',
        hover='#25282E', active='#2D3138', selected='#1F2C42', selected_quiet='#272B33',
        hairline='#2A2D34', strong='#3A3F48', control='#6F7783', focus='#6A9BFF',
        text='#E6E8EC', text2='#AEB3BD', muted='#8E949F', disabled='#5C626C',
        accent='#3D6FD6', accent_hover='#3463C4', on_accent='#FFFFFF', accent_fg='#8DB5FF',
        working='#62B3E6', attention='#E5B565', done='#62BF8F', failed='#F28B82', idle='#8E949F',
        working_bg='#1A2C38', attention_bg='#352A17', done_bg='#183126', failed_bg='#3A2022',
        add_bg='#18291F', del_bg='#33191C', add_fg='#7DCB9B', del_fg='#F29A92',
        term_bg='#1B1D22', term_fg='#DADDE3',
    ),
    'light': dict(
        app='#F3F4F6', sidebar='#F3F4F6', panel='#FFFFFF', raised='#FFFFFF', overlay='#FFFFFF',
        hover='#E8EAED', active='#DEE1E6', selected='#DFE8FA', selected_quiet='#E6E8EC',
        hairline='#E1E3E8', strong='#CBD0D7', control='#868D98', focus='#2F6BDB',
        text='#1C1F24', text2='#4C525C', muted='#5F6672', disabled='#A2A8B1',
        accent='#2F66D0', accent_hover='#2959B8', on_accent='#FFFFFF', accent_fg='#2459BF',
        working='#136894', attention='#8F5A00', done='#1A7048', failed='#BF3434', idle='#5F6672',
        working_bg='#E3F1F8', attention_bg='#FBF0DC', done_bg='#E2F3EA', failed_bg='#FBE7E6',
        add_bg='#E6F4EA', del_bg='#FCEBEA', add_fg='#1C6E43', del_fg='#B02E2E',
        term_bg='#FFFFFF', term_fg='#23272E',
    ),
    'hc-dark': dict(
        app='#000000', sidebar='#000000', panel='#000000', raised='#0A0A0A', overlay='#0A0A0A',
        hover='#1F1F1F', active='#2B2B2B', selected='#0A2A5C', selected_quiet='#1F1F1F',
        hairline='#8A8A8A', strong='#FFFFFF', control='#FFFFFF', focus='#FFD60A',
        text='#FFFFFF', text2='#F0F0F0', muted='#D6D6D6', disabled='#8A8A8A',
        accent='#1A5FD0', accent_hover='#2A6FE0', on_accent='#FFFFFF', accent_fg='#9CC3FF',
        working='#8CD3FF', attention='#FFD45C', done='#7EE8AE', failed='#FF9C9C', idle='#D6D6D6',
        working_bg='#000000', attention_bg='#000000', done_bg='#000000', failed_bg='#000000',
        add_bg='#002A12', del_bg='#3A0008', add_fg='#7EE8AE', del_fg='#FF9C9C',
        term_bg='#000000', term_fg='#FFFFFF',
    ),
    'hc-light': dict(
        app='#FFFFFF', sidebar='#FFFFFF', panel='#FFFFFF', raised='#FFFFFF', overlay='#FFFFFF',
        hover='#E6E6E6', active='#D4D4D4', selected='#CCE0FF', selected_quiet='#E6E6E6',
        hairline='#5A5A5A', strong='#000000', control='#000000', focus='#0037A6',
        text='#000000', text2='#141414', muted='#262626', disabled='#6E6E6E',
        accent='#0040B0', accent_hover='#00368F', on_accent='#FFFFFF', accent_fg='#0037A6',
        working='#004E80', attention='#5C3A00', done='#00562F', failed='#9A0010', idle='#262626',
        working_bg='#FFFFFF', attention_bg='#FFFFFF', done_bg='#FFFFFF', failed_bg='#FFFFFF',
        add_bg='#D9F2E1', del_bg='#FBDCDC', add_fg='#00562F', del_fg='#9A0010',
        term_bg='#FFFFFF', term_fg='#000000',
    ),
}

B = {
    'dark': dict(
        app='#101114', sidebar='#101114', panel='#191A1F', raised='#212228', overlay='#26272E',
        hover='#1C1D22', active='#26272E', selected='#26264A', selected_quiet='#22232A',
        hairline='#2A2B33', strong='#383A44', control='#727585', focus='#9EA1FF',
        text='#ECECF0', text2='#ABACB6', muted='#90919C', disabled='#5E5F6A',
        accent='#5B5ED8', accent_hover='#5154CC', on_accent='#FFFFFF', accent_fg='#A9ABFF',
        working='#6DB3EE', attention='#EBAE62', done='#5CC595', failed='#F38B90', idle='#90919C',
        working_bg='#1B2A3A', attention_bg='#382B19', done_bg='#173126', failed_bg='#3B1F24',
        add_bg='#17291F', del_bg='#331A1E', add_fg='#7DCB9B', del_fg='#F29A9C',
        term_bg='#191A1F', term_fg='#DEDEE4',
    ),
    'light': dict(
        app='#ECEDF1', sidebar='#ECEDF1', panel='#FFFFFF', raised='#FFFFFF', overlay='#FFFFFF',
        hover='#E2E3E9', active='#D8DAE1', selected='#FFFFFF', selected_quiet='#F6F6F9',
        hairline='#E7E8ED', strong='#D2D4DC', control='#868896', focus='#4F52CC',
        text='#1B1C21', text2='#4D4F5A', muted='#5F616C', disabled='#A4A6B0',
        accent='#4F52CC', accent_hover='#4447B5', on_accent='#FFFFFF', accent_fg='#4346BE',
        working='#13699F', attention='#8C5700', done='#18704A', failed='#B62D37', idle='#5F616C',
        working_bg='#E2EFF8', attention_bg='#FAEEDA', done_bg='#E1F2E9', failed_bg='#FAE6E7',
        add_bg='#E6F4EA', del_bg='#FCEBEB', add_fg='#1C6E43', del_fg='#B02E33',
        term_bg='#FFFFFF', term_fg='#24252B',
    ),
}

C = {
    'dark': dict(
        app='#0E1117', sidebar='#12161E', panel='#151A22', raised='#1B212C', overlay='#212836',
        hover='#1C2330', active='#243042', selected='#14333A', selected_quiet='#1E2531',
        hairline='#232B38', strong='#334055', control='#6C7A90', focus='#3FC7B4',
        text='#E4E9F1', text2='#A6B1C2', muted='#8C98AB', disabled='#566174',
        accent='#11806F', accent_hover='#0D7062', on_accent='#FFFFFF', accent_fg='#4FD2BE',
        working='#72AEFF', attention='#F3B556', done='#55C68E', failed='#FF8790', idle='#8C98AB',
        working_bg='#16263F', attention_bg='#3A2C14', done_bg='#14301F', failed_bg='#3D1C22',
        add_bg='#14291C', del_bg='#33181C', add_fg='#7DCB9B', del_fg='#F29A9C',
        term_bg='#0F131A', term_fg='#DCE2EC',
    ),
    'light': dict(
        app='#EDF0F4', sidebar='#F7F8FA', panel='#FFFFFF', raised='#FFFFFF', overlay='#FFFFFF',
        hover='#E4E8EE', active='#D9DFE7', selected='#DAF1EC', selected_quiet='#E8ECF1',
        hairline='#DCE1E8', strong='#C6CED9', control='#7F8A9A', focus='#0F766E',
        text='#18202B', text2='#495466', muted='#5B6677', disabled='#9EA8B6',
        accent='#0F766E', accent_hover='#0C665F', on_accent='#FFFFFF', accent_fg='#0B6B63',
        working='#1D62C4', attention='#935600', done='#18774A', failed='#BE2F3B', idle='#5B6677',
        working_bg='#E3EDFB', attention_bg='#FBEEDA', done_bg='#E0F2E8', failed_bg='#FBE6E8',
        add_bg='#E6F4EA', del_bg='#FCEBEB', add_fg='#1C6E43', del_fg='#B02E33',
        term_bg='#FFFFFF', term_fg='#1E2632',
    ),
}

DIRECTIONS = {'A': A, 'B': B, 'C': C}

# Metric tokens per direction (logical px at 100%).
METRICS = {
    'A': dict(row=28, row_comfortable=32, worker_row=28, titlebar=36, tabbar=32, panel_header=30,
              statusbar=24, icon=16, icon_small=14, stroke=1.5, radius_control=4, radius_card=6,
              radius_overlay=8, sidebar_w=248, gap=0, base_font=13, line=20),
    'B': dict(row=30, row_comfortable=34, worker_row=28, titlebar=40, tabbar=34, panel_header=32,
              statusbar=26, icon=16, icon_small=16, stroke=0, radius_control=6, radius_card=8,
              radius_overlay=12, sidebar_w=256, gap=6, base_font=13, line=20),
    'C': dict(row=32, row_comfortable=36, worker_row=28, titlebar=36, tabbar=32, panel_header=32,
              statusbar=0, icon=16, icon_small=14, stroke=0, radius_control=6, radius_card=8,
              radius_overlay=10, sidebar_w=292, gap=4, base_font=13, line=20),
}
