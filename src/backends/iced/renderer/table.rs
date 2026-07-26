use iced::{Length, widget};

use crate::backends::iced::{
    MarkWidget,
    dom::DomRef,
    renderer::ValidTheme,
    structs::{ChildAlignment, ChildData, ChildDataFlags, RenderedSpan},
};

impl<'a, M: Clone + 'static, T: ValidTheme + 'a> MarkWidget<'a, M, T>
where
    <T as widget::button::Catalog>::Class<'a>: From<widget::button::StyleFn<'a, T>>,
{
    pub(crate) fn draw_table(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        let mut header_cells: Vec<TableCell<'a, M, T>> = Vec::new();
        let mut column_alignments: Vec<Option<ChildAlignment>> = Vec::new();
        let mut body_rows: Vec<Vec<TableCell<'a, M, T>>> = Vec::new();

        for section in node.children_iter() {
            let Some(section_name) = section.tag_name() else {
                continue;
            };

            let rows: Vec<_> = if section_name == "tr" {
                vec![section]
            } else {
                section
                    .children_iter()
                    .filter(|row| row.tag_name() == Some("tr"))
                    .collect()
            };

            for row in rows {
                let cells: Vec<_> = row
                    .children_iter()
                    .filter(|cell| matches!(cell.tag_name(), Some("th" | "td")))
                    .collect();
                if cells.is_empty() {
                    continue;
                }

                let is_header_row = section_name == "thead"
                    || cells.iter().any(|cell| cell.tag_name() == Some("th"));

                if is_header_row && header_cells.is_empty() {
                    self.table_add_header_cell(
                        data,
                        &mut header_cells,
                        &mut column_alignments,
                        &cells,
                    );
                } else {
                    if column_alignments.is_empty() {
                        column_alignments =
                            cells.iter().map(|cell| cell_alignment(*cell)).collect();
                    }
                    body_rows.push(
                        cells
                            .into_iter()
                            .map(|cell| TableCell {
                                content: self.render_children(cell, data),
                                width: cell_width(cell, self.text_size),
                            })
                            .collect(),
                    );
                }
            }
        }

        let body: iced::Element<'a, M, T> = widget::column(
            body_rows
                .into_iter()
                .map(|row| draw_row(row, &column_alignments).into()),
        )
        .spacing(2)
        .into();

        if header_cells.is_empty() {
            body.into()
        } else {
            widget::column![
                draw_row(header_cells, &column_alignments),
                widget::rule::horizontal(1),
                body,
            ]
            .spacing(4)
            .into()
        }
    }

    fn table_add_header_cell(
        &mut self,
        data: ChildData,
        header_cells: &mut Vec<TableCell<'a, M, T>>,
        column_alignments: &mut Vec<Option<ChildAlignment>>,
        cells: &[DomRef<'_>],
    ) {
        *column_alignments = cells.iter().map(|cell| cell_alignment(*cell)).collect();

        *header_cells = cells
            .iter()
            .map(|cell| TableCell {
                content: self.render_children(*cell, data.insert(ChildDataFlags::BOLD)),
                width: cell_width(*cell, self.text_size),
            })
            .collect();
    }
}

struct TableCell<'a, M: Clone + 'static, T: ValidTheme + 'a> {
    content: RenderedSpan<'a, M, T>,
    width: Option<f32>,
}

fn draw_row<'a, M: Clone + 'static, T: ValidTheme + 'a>(
    cells: Vec<TableCell<'a, M, T>>,
    column_alignments: &[Option<ChildAlignment>],
) -> widget::Row<'a, M, T> {
    widget::row(cells.into_iter().enumerate().map(|(i, cell)| {
        make_cell(
            cell.content,
            column_alignments.get(i).copied().flatten(),
            cell.width,
        )
        .into()
    }))
    .spacing(2)
}

fn make_cell<'a, M: Clone + 'static, T: ValidTheme + 'a>(
    content: RenderedSpan<'a, M, T>,
    align: Option<ChildAlignment>,
    width: Option<f32>,
) -> widget::Column<'a, M, T> {
    let alignment: iced::Alignment = align.map_or(iced::Alignment::Start, ChildAlignment::into);
    let width = width.map_or(Length::Fill, Length::Fixed);

    widget::column![content.render()]
        .align_x(alignment)
        .padding(5)
        .width(width)
}

fn cell_alignment(cell: DomRef<'_>) -> Option<ChildAlignment> {
    let mut align = None;
    cell.for_each_attr(|name, value| {
        if name == "align" {
            align = match value {
                "right" => Some(ChildAlignment::Right),
                "center" | "centre" => Some(ChildAlignment::Center),
                _ => None,
            };
        }
    });
    align
}

fn cell_width(cell: DomRef<'_>, font_size: f32) -> Option<f32> {
    cell.get_attr("width")
        .and_then(|n| n.parse::<f32>().ok())
        .or_else(|| {
            cell.get_attr("style")
                .and_then(|s| super::css_dimension_with_font(s, "width", font_size))
        })
}
