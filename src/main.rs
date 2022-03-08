use glutin_window::GlutinWindow as Window;
//use graphics;
use jack;
use opengl_graphics::{GlGraphics, OpenGL};
use piston::event_loop::{EventSettings, Events};
use piston::input::{Event, Input};
use piston::input::{RenderArgs, RenderEvent, UpdateArgs, UpdateEvent};
use piston::window::WindowSettings;
use ringbuf::{Consumer, Producer, RingBuffer};

const FRAME_SIZE: usize = 1024;

struct JackProc {
    in_1: jack::Port<jack::AudioIn>,
    in_2: jack::Port<jack::AudioIn>,
    producer_1: Producer<f32>,
    producer_2: Producer<f32>,
}

impl jack::ProcessHandler for JackProc {
    fn process(&mut self, _: &jack::Client, ps: &jack::ProcessScope) -> jack::Control {
        let in_1_p = self.in_1.as_slice(ps);
        let in_2_p = self.in_2.as_slice(ps);
        let mut got_err = false;
        for smpl in in_1_p.iter() {
            match self.producer_1.push(*smpl) {
                Ok(_) => (),
                Err(_) => got_err = true,
            }
        }
        for smpl in in_2_p.iter() {
            match self.producer_2.push(*smpl) {
                Ok(_) => (),
                Err(_) => got_err = true,
            }
        }
        if got_err {
            println!("got err");
        }
        jack::Control::Continue
    }
}

fn norm(a: f32, b: f32) -> f32 {
    (a.powi(2) + b.powi(2)).sqrt()
}

fn heaviside(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else {
        0.0
    }
}

fn recurrence_matrix(e: f32, vec: &[f32]) -> Vec<Vec<f32>> {
    let mut matrix = vec![];
    for item_i in vec.iter() {
        let mut row = vec![];
        for item_j in vec.iter() {
            //            row.push(heaviside(e - (item_i - item_j).abs()))
            //            row.push(norm(*item_i, *item_j))
            row.push((item_i - item_j).abs())
            //            row.push((item_i - item_j).powi(2))
        }
        matrix.push(row)
    }
    matrix
}

fn recurrence_matrix2(e: f32, vec1: &[f32], vec2: &[f32]) -> Vec<Vec<f32>> {
    let mut matrix = vec![];
    for item_i in vec1.iter() {
        let mut row = vec![];
        for item_j in vec2.iter() {
            //            row.push(heaviside(e - (item_i - item_j).abs()))
            //            row.push(norm(*item_i, *item_j))
            row.push((item_i - item_j).abs())
            //            row.push((item_i - item_j).powi(2))
        }
        matrix.push(row)
    }
    matrix
}

struct FilteredBuffer {
    target_length: usize,
    chunk_size: usize,
    buffer: Vec<f32>,
    rec_matrix: Vec<Vec<f32>>,
}

impl FilteredBuffer {
    fn new(target_length: usize, chunk_size: usize) -> Self {
        FilteredBuffer {
            target_length,
            chunk_size,
            buffer: vec![],
            rec_matrix: vec![],
        }
    }

    fn input(&mut self, input_buffer: &[f32]) {
        //println!("chunk {} first smpl {}", self.chunk_size, input_buffer[0]);

        // let mut averaged_input: Vec<f32> = input_buffer
        //     .chunks(self.chunk_size)
        //     .map(|c| {
        //         let sum: f32 = c.iter().sum();
        //         sum / (self.chunk_size as f32)
        //     })
        //     .collect();

        // self.previous = if self.buffer.len() > 0 {
        //     self.buffer[self.buffer.len() - 1]
        // } else {
        //     0.0
        // };
        let mut averaged_input: Vec<f32> = input_buffer
            .iter()
            .step_by(self.chunk_size)
            .map(|f| *f)
            .collect();

        self.buffer.append(&mut averaged_input);

        //      println!("size after {}", self.buffer.len());
        let too_many = self.buffer.len() as isize - self.target_length as isize;
        if too_many > 0 {
            //            self.buffer = self.buffer[(too_many as usize)..].to_vec();
            self.buffer.drain(0..(too_many as usize));
        }
    }

    // fn is_complete(&self) -> bool {
    //     self.target_length == self.buffer.len()
    // }
}

enum Mode {
    XY,
    Recurrence,
}

pub struct App {
    gl: GlGraphics, // OpenGL drawing backend.
    buffer1: Vec<f32>,
    buffer2: Vec<f32>,
    filtered_buffer1: FilteredBuffer,
    filtered_buffer2: FilteredBuffer,
    mode: Mode,
    factor: f32,
    exponent: f32,
}

impl App {
    fn render(&mut self, args: &RenderArgs) {
        use graphics::*;

        let filtered_buffer1 = &self.filtered_buffer1;
        let filtered_buffer2 = &self.filtered_buffer2;
        let mode = &self.mode;
        let factor = self.factor;
        let exponent = self.exponent;
        
        self.gl.draw(args.viewport(), |c, gl| {
            clear(graphics::color::BLACK, gl);

            match mode {
                Mode::XY => {
                    if filtered_buffer1.buffer.len() > 1 {
                        for i in 0..(filtered_buffer1.buffer.len() - 1) {
                            let x1 = filtered_buffer1.buffer[i];
                            let x2 = filtered_buffer1.buffer[i + 1];
                            let y1 = filtered_buffer2.buffer[i];
                            let y2 = filtered_buffer2.buffer[i + 1];

                            let xpos1 = args.window_size[0] as f32 * ((x1 + 1.0) / 2.0);
                            let ypos1 = args.window_size[1] as f32 * ((y1 + 1.0) / 2.0);
                            let xpos2 = args.window_size[0] as f32 * ((x2 + 1.0) / 2.0);
                            let ypos2 = args.window_size[1] as f32 * ((y2 + 1.0) / 2.0);

                            let l = [xpos1 as f64, ypos1 as f64, xpos2 as f64, ypos2 as f64];
                            line(color::alpha(1.0), 1.0, l, c.transform, gl);
                        }
                    }
                }
                Mode::Recurrence => {
                    let length = filtered_buffer1.rec_matrix.len() as f64;
                    let xfac = args.window_size[0] / length;
                    let yfac = args.window_size[1] / length;

                    // let g = color::alpha(1.0);
                    // let r = rectangle::rectangle_by_corners(
                    //     10.0 * xfac,
                    //     (length - 1.0) * yfac,
                    //     11.0 * xfac,
                    //     length * yfac,
                    // );
                    // rectangle(g, r, c.transform, gl);

                    filtered_buffer1
                        .rec_matrix
                        .iter()
                        .enumerate()
                        .for_each(|(i, vec)| {
                            vec.iter().enumerate().for_each(|(j, val)| {
                                let g = color::alpha( (*val * factor).powf(exponent) );

                                //let y = (args.window_size[1] / 2.0) + (80 * filter_idx) as f64;
                                //                let r = Rectangle::new(g);
                                let r = rectangle::rectangle_by_corners(
                                    i as f64 * xfac,
                                    j as f64 * yfac,
                                    (i + 1) as f64 * xfac,
                                    (j + 1) as f64 * yfac,
                                );
                                rectangle(g, r, c.transform, gl);
                            })
                        })
                }
            }
        });
    }

    fn update(
        &mut self,
        _args: &UpdateArgs,
        consumer1: &mut Consumer<f32>,
        consumer2: &mut Consumer<f32>,
    ) {
        self.buffer1 = vec![];
        self.buffer2 = vec![];

        let length = (*consumer1).len();
        if length > 0 {
            for _i in 0..length {
                if let Some(f) = (*consumer1).pop() {
                    self.buffer1.push(f);
                }
            }
        }

        let length = (*consumer2).len();
        if length > 0 {
            for _i in 0..length {
                if let Some(f) = (*consumer2).pop() {
                    self.buffer2.push(f);
                }
            }
        }

        let buf1 = &self.buffer1;
        let buf2 = &self.buffer2;

        let e = 0.1;
        self.filtered_buffer1.input(&buf1);
        self.filtered_buffer2.input(&buf2);

        self.filtered_buffer1.rec_matrix = recurrence_matrix2(
            e,
            &self.filtered_buffer1.buffer,
            &self.filtered_buffer2.buffer,
        );

        // self.filtered_buffer2.rec_matrix = recurrence_matrix2(
        //     e,
        //     &self.filtered_buffer1.buffer,
        //     &self.filtered_buffer2.buffer,
        // );
    }
}

fn main() {
    let (client, _status) =
        jack::Client::new("visualizer", jack::ClientOptions::NO_START_SERVER).unwrap();

    let in_1 = client
        .register_port("vis_in_1", jack::AudioIn::default())
        .unwrap();

    let in_2 = client
        .register_port("vis_in_2", jack::AudioIn::default())
        .unwrap();

    let ring_buffer_1 = RingBuffer::<f32>::new(FRAME_SIZE * 10);
    let ring_buffer_2 = RingBuffer::<f32>::new(FRAME_SIZE * 10);

    let (producer_1, mut consumer_1) = ring_buffer_1.split();
    let (producer_2, mut consumer_2) = ring_buffer_2.split();

    let process = JackProc {
        in_1,
        in_2,
        producer_1,
        producer_2,
    };

    let _active_client = client.activate_async((), process).unwrap();

    // Change this to OpenGL::V2_1 if not working.
    let opengl = OpenGL::V3_2;

    // Create an Glutin window.
    let mut window: Window = WindowSettings::new("visualizer", [1024, 768])
        .graphics_api(opengl)
        .exit_on_esc(true)
        .build()
        .unwrap();

    // Create a new game and run it.
    let mut app = App {
        gl: GlGraphics::new(opengl),
        buffer1: vec![],
        buffer2: vec![],
        filtered_buffer1: FilteredBuffer::new(800, 1),
        filtered_buffer2: FilteredBuffer::new(800, 1),
        mode: Mode::XY,
        factor: 1.0,
        exponent: 1.0,
    };

    let settings = EventSettings::new();
    //    settings.ups = 30;
    let mut events = Events::new(settings);
    while let Some(e) = events.next(&mut window) {
        match &e {
            Event::Input(Input::Text(t), _) => match t.as_str() {
                "1" => app.mode = Mode::XY,
                "2" => app.mode = Mode::Recurrence,
                "3" => app.factor = app.factor - 0.1,
                "4" => app.factor = app.factor + 0.1,
                "5" => app.exponent = app.exponent - 0.1,
                "6" => app.exponent = app.exponent + 0.1,
                "7" => println!("factor: {}, exponent: {}", app.factor, app.exponent),
                _ => (),
            },
            _ => (),
        }

        if let Some(args) = e.render_args() {
            app.render(&args);
        }

        if let Some(args) = e.update_args() {
            app.update(&args, &mut consumer_1, &mut consumer_2);
        }
    }
}
