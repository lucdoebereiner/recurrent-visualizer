use glutin_window::GlutinWindow as Window;
//use graphics;
use jack;
use opengl_graphics::{GlGraphics, OpenGL};
use piston::event_loop::{EventSettings, Events};
use piston::input::{Event, Input};
use piston::input::{RenderArgs, RenderEvent, UpdateArgs, UpdateEvent};
use piston::window::WindowSettings;
use ringbuf::{Consumer, Producer, RingBuffer};
use rosc::OscPacket;
//use std::env;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
//use std::str::FromStr;
use std::sync::mpsc;
use std::thread;

const FRAME_SIZE: usize = 1024;

struct JackProc {
    in_1: jack::Port<jack::AudioIn>,
    in_2: jack::Port<jack::AudioIn>,
    in_3: jack::Port<jack::AudioIn>,
    producer_1: Producer<f32>,
    producer_2: Producer<f32>,
    producer_3: Producer<f32>,
}

impl jack::ProcessHandler for JackProc {
    fn process(&mut self, _: &jack::Client, ps: &jack::ProcessScope) -> jack::Control {
        let in_1_p = self.in_1.as_slice(ps);
        let in_2_p = self.in_2.as_slice(ps);
        let in_3_p = self.in_3.as_slice(ps);
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
        for smpl in in_3_p.iter() {
            match self.producer_3.push(*smpl) {
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
    let mut matrix: Vec<Vec<f32>> = vec![];
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

fn lag2ud(current: f32, target: f32, factor_up: f32, factor_down: f32) -> f32 {
    if (current - target).abs() < 0.00001 {
        target
    } else {
        if target > current {
            target + factor_up * (current - target)
        } else {
            target + factor_down * (current - target)
        }
    }
}

// non symmetrical excerpt
fn recurrence_matrix2square(
    e: f32,
    vec1: &[f32],
    vec2: &[f32],
    matrix: &mut [[f32; 400]; 400],
    down_fac: f32,
    up_fac: f32,
) {
    let n = vec1.len() / 2;
    for i in 0..n {
        for j in n..(n * 2) {
            //            row.push(heaviside(e - (item_i - item_j).abs()))
            //            row.push(norm(*item_i, *item_j))
            matrix[i][j - n] = lag2ud(
                matrix[i][j - n],
                (vec1[i] - vec2[j]).abs(),
                up_fac,
                down_fac,
            );
            //            row.push((item_i - item_j).powi(2))
        }
    }
}

fn recurrence_matrix2rev(e: f32, vec1: &[f32], vec2: &[f32]) -> Vec<Vec<f32>> {
    let mut matrix: Vec<Vec<f32>> = vec![];
    for item_i in vec1.iter() {
        let mut row = vec![];
        for item_j in vec2.iter().rev() {
            //            row.push(heaviside(e - (item_i - item_j).abs()))
            //            row.push(norm(*item_i, *item_j))
            row.push((item_i - item_j).abs())
            //            row.push((item_i - item_j).powi(2))
        }
        matrix.push(row)
    }
    matrix
}

fn recurrence_matrix3(e: f32, vec1: &[f32], vec2: &[f32], vec3: &[f32]) -> Vec<Vec<f32>> {
    let mut matrix: Vec<Vec<f32>> = vec![];
    let n = vec1.len();
    for xi in 0..n {
        let mut column = vec![];
        for yi in 0..n {
            let val = (vec1[xi] - vec2[yi]).abs()
                * (vec1[xi] - vec3[yi]).abs()
                * (vec3[xi] - vec2[yi]).abs();
            column.push(val);
            // println!("{} {:?} {:?}\n", i, vec2[i], vec3[i]);
        }
        //println!("{:?}\n\n", row);
        matrix.push(column)
    }
    matrix
}

fn recurrence_matrix3interp(e: f32, vec1: &[f32], vec2: &[f32], vec3: &[f32]) -> Vec<Vec<f32>> {
    let mut matrix: Vec<Vec<f32>> = vec![];
    let n = vec1.len();
    for i in 0..n {
        let mut row = vec![];
        let ratio = i as f32 / n as f32;
        let idx_x = i;
        let x = if ratio < 2.0 / 3.0 {
            vec1[idx_x]
        } else {
            let adjusted_ratio = (ratio - 2.0 / 3.0) * 3.0;
            vec1[idx_x] * (1.0 - adjusted_ratio) + vec3[idx_x] * adjusted_ratio
        };
        for j in 0..n {
            let idx_y = n - 1 - j;
            let y = if ratio > 1.0 / 3.0 {
                vec3[idx_y]
            } else {
                vec2[idx_y] * (1.0 - (ratio * 3.0)) + vec3[idx_y] * (ratio * 3.0)
            };
            row.push((x - y).abs())
        }
        matrix.push(row)
    }

    // for i in 0..n {
    //     let mut row = vec![];
    //     let ratio = i as f32 / n as f32;
    //     let x = (vec1[i] * (1.0 - ratio)) + (vec2[i] * ratio);
    //     for j in 0..n {
    //         let y = vec3[j];
    //         row.push((x - y).abs())
    //     }
    //     matrix.push(row)
    // }
    /*  let total_n = n * 2;
    for i in 0..total_n {
        let mut row = vec![];
        let x = if i < n {
            let ratio = i as f32 / n as f32;
            (vec1[i] * (1.0 - ratio)) + (vec2[i] * ratio)
        } else {
            let ratio = (i - n) as f32 / n as f32;
            (vec2[n - (i - n) - 1] * (1.0 - ratio)) + (vec3[n - (i - n) - 1] * ratio)
        };
        for j in 0..total_n {
            let y = if j < n {
                let ratio = j as f32 / n as f32;
                (vec3[n - j - 1] * (1.0 - ratio)) + (vec2[n - j - 1] * ratio)
            } else {
                let ratio = (j - n) as f32 / n as f32;
                (vec2[j - n] * (1.0 - ratio)) + (vec1[j - n] * ratio)
            };
            row.push((x - y).abs())
        }
        matrix.push(row)
    } */
    matrix
}

struct FilteredBuffer {
    target_length: usize,
    chunk_size: usize,
    buffer: Vec<f32>,
    //rec_matrix: Vec<Vec<f32>>,
}

impl FilteredBuffer {
    fn new(target_length: usize, chunk_size: usize) -> Self {
        FilteredBuffer {
            target_length,
            chunk_size,
            buffer: vec![],
            // rec_matrix: vec![],
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
    RecurrenceRev,
    RecurrenceThreeMulti,
    RecurrenceThreeSum,
    RecurrenceThreeColor,
}

pub struct App {
    gl: GlGraphics, // OpenGL drawing backend.
    buffer1: Vec<f32>,
    buffer2: Vec<f32>,
    buffer3: Vec<f32>,
    filtered_buffer1: FilteredBuffer,
    filtered_buffer2: FilteredBuffer,
    filtered_buffer3: FilteredBuffer,
    mode: Mode,
    factor: f32,
    exponent: f32,
    rec_matrix2darray: [[f32; 400]; 400],
    rec_matrix2d: Vec<Vec<f32>>,
    rec_matrix3d: Vec<Vec<f32>>,
    down_fac: f32,
    up_fac: f32,
    bwmode: u8,
    offset_x: i32,
    offset_y: i32,
    zoom: f32,
}

impl App {
    fn render(&mut self, args: &RenderArgs) {
        use graphics::*;

        let filtered_buffer1 = &self.filtered_buffer1;
        let filtered_buffer2 = &self.filtered_buffer2;
        let matrix2d = &self.rec_matrix2d;
        let matrix2darray = &self.rec_matrix2darray;
        let matrix3d = &self.rec_matrix3d;
        let mode = &self.mode;
        let factor = self.factor;
        let exponent = self.exponent;
        let bwmode = &self.bwmode;
        let zoom = &self.zoom;
        let offset_x = &self.offset_x;
        let offset_y = &self.offset_y;

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

                Mode::Recurrence
                | Mode::RecurrenceThreeColor
                | Mode::RecurrenceThreeMulti
                | Mode::RecurrenceRev
                | Mode::RecurrenceThreeSum => {
                    let n = (matrix3d.len() as f32 / (*zoom).max(1.0)).ceil();
                    let length = n as f64;
                    let xfac = args.window_size[0] / length;
                    let yfac = args.window_size[1] / length;

                    let to_skip_x = (matrix3d.len() - n as usize).min(*offset_x as usize);
                    let to_skip_y = (matrix3d.len() - n as usize).min(*offset_y as usize);
                    // println!("to skip {}, offset {}", to_skip, offset);

                    matrix3d
                        .iter()
                        .skip(to_skip_x)
                        .take(n as usize)
                        .enumerate()
                        .for_each(|(i, vec)| {
                            vec.iter()
                                .skip(to_skip_y)
                                .take(n as usize)
                                .enumerate()
                                .for_each(|(j, val)| {
                                    let mut value = *val * factor;
                                    if *bwmode == 1 {
                                        value = 1.0 - value;
                                    }
                                    let g = color::alpha(value.powf(exponent));

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
                } /*   Mode::RecurrenceThreeColor
                  | Mode::RecurrenceThreeMulti
                  | Mode::RecurrenceThreeSum => {
                      let length = matrix3d.len() as f64;
                      let xfac = args.window_size[0] / length;
                      let yfac = args.window_size[1] / length;

                      matrix3d.iter().enumerate().for_each(|(i, vec)| {
                          vec.iter().enumerate().for_each(|(j, val)| {
                              //let g = color::alpha((val.0 * val.1 * val.2 * factor).powf(exponent));
                              // let g = color::alpha(((val.0 + val.1 + val.2) * factor).powf(exponent));
                              let g = [val.0, val.1, val.2, 1.0];
                              //let g = [1.0, 0.0, 0.0, 1.0];
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
                  } */
            }
        });
    }

    fn update(
        &mut self,
        _args: &UpdateArgs,
        consumer1: &mut Consumer<f32>,
        consumer2: &mut Consumer<f32>,
        consumer3: &mut Consumer<f32>,
        down_fac: f32,
        up_fac: f32,
    ) {
        self.buffer1 = vec![];
        self.buffer2 = vec![];
        self.buffer3 = vec![];

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
                    //print!("2: {}\n", f)
                }
            }
        }

        let length = (*consumer3).len();
        if length > 0 {
            for _i in 0..length {
                if let Some(f) = (*consumer3).pop() {
                    self.buffer3.push(f);
                    //print!("3: {}\n", f)
                }
            }
        }

        let buf1 = &self.buffer1;
        let buf2 = &self.buffer2;
        let buf3 = &self.buffer3;

        let e = 0.1;
        self.filtered_buffer1.input(&buf1);
        self.filtered_buffer2.input(&buf2);
        self.filtered_buffer3.input(&buf3);

        let mode = &self.mode;

        match mode {
            Mode::Recurrence | Mode::XY => {
                self.rec_matrix2d = recurrence_matrix2(
                    e,
                    &self.filtered_buffer1.buffer,
                    &self.filtered_buffer2.buffer,
                )
            }
            Mode::RecurrenceRev => {
                self.rec_matrix2d = recurrence_matrix2rev(
                    e,
                    &self.filtered_buffer1.buffer,
                    &self.filtered_buffer2.buffer,
                )
            }
            Mode::RecurrenceThreeMulti | Mode::RecurrenceThreeColor | Mode::RecurrenceThreeSum => {
                self.rec_matrix3d = recurrence_matrix3(
                    e,
                    &self.filtered_buffer1.buffer,
                    &self.filtered_buffer2.buffer,
                    &self.filtered_buffer3.buffer,
                );
                /*              recurrence_matrix2square(
                    e,
                    &self.filtered_buffer1.buffer,
                    &self.filtered_buffer2.buffer,
                    &mut self.rec_matrix2darray,
                    down_fac,
                    up_fac,
                ); */
                //  self.rec_matrix2d = recurrence_matrix2(
                //   e,
                // &self.filtered_buffer1.buffer,
                //&self.filtered_buffer2.buffer,
                // &mut self.rec_matrix2darray,
                //down_fac,
                //up_fac,
                //);
                //  &self.filtered_buffer3.buffer,
            }
        }
    }
}

fn handle_packet(packet: OscPacket) {
    match packet {
        OscPacket::Message(msg) => {
            println!("OSC address: {}", msg.addr);
            println!("OSC arguments: {:?}", msg.args);
        }
        OscPacket::Bundle(bundle) => {
            println!("OSC Bundle: {:?}", bundle);
        }
    }
}

fn float_osc(p: OscPacket) -> Option<f32> {
    if let OscPacket::Message(msg) = p {
        msg.args
            .first()
            .and_then(move |v| rosc::OscType::float(v.clone()))
    } else {
        None
    }
}

fn int_osc(p: OscPacket) -> Option<i32> {
    if let OscPacket::Message(msg) = p {
        msg.args
            .first()
            .and_then(move |v| rosc::OscType::int(v.clone()))
    } else {
        None
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

    let in_3 = client
        .register_port("vis_in_3", jack::AudioIn::default())
        .unwrap();

    let ring_buffer_1 = RingBuffer::<f32>::new(FRAME_SIZE * 10);
    let ring_buffer_2 = RingBuffer::<f32>::new(FRAME_SIZE * 10);
    let ring_buffer_3 = RingBuffer::<f32>::new(FRAME_SIZE * 10);

    let (producer_1, mut consumer_1) = ring_buffer_1.split();
    let (producer_2, mut consumer_2) = ring_buffer_2.split();
    let (producer_3, mut consumer_3) = ring_buffer_3.split();

    let process = JackProc {
        in_1,
        in_2,
        in_3,
        producer_1,
        producer_2,
        producer_3,
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
    let mut app: App = App {
        gl: GlGraphics::new(opengl),
        buffer1: vec![],
        buffer2: vec![],
        buffer3: vec![],
        filtered_buffer1: FilteredBuffer::new(1000, 1),
        filtered_buffer2: FilteredBuffer::new(1000, 1),
        filtered_buffer3: FilteredBuffer::new(1000, 1),
        mode: Mode::Recurrence,
        factor: 1.0,
        exponent: 1.0,
        rec_matrix2darray: [[0.0; 400]; 400],
        rec_matrix2d: vec![],
        rec_matrix3d: vec![],
        down_fac: 0.9,
        up_fac: 0.1,
        bwmode: 0,
        offset_x: 0,
        offset_y: 0,
        zoom: 1.0,
    };

    let mut settings = EventSettings::new();
    settings.max_fps = 30;
    //    settings.ups = 30;
    let mut events = Events::new(settings);

    let addr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8000);
    let sock = UdpSocket::bind(addr).unwrap();
    println!("Listening to {}", addr);
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let mut buf = [0u8; rosc::decoder::MTU];

        loop {
            match sock.recv_from(&mut buf) {
                Ok((size, addr)) => {
                    //println!("Received packet with size {} from: {}", size, addr);
                    let (_, packet) = rosc::decoder::decode_udp(&buf[..size]).unwrap();
                    tx.send(packet).unwrap();
                    //handle_packet(packet);
                }
                Err(e) => {
                    println!("Error receiving from socket: {}", e);
                    break;
                }
            }
        }
    });

    while let Some(e) = events.next(&mut window) {
        match &e {
            Event::Input(Input::Text(t), _) => match t.as_str() {
                "1" => app.mode = Mode::XY,
                "2" => app.mode = Mode::Recurrence,
                "3" => app.mode = Mode::RecurrenceThreeColor,
                "4" => app.mode = Mode::RecurrenceThreeMulti,
                "5" => app.mode = Mode::RecurrenceThreeSum,
                "6" => app.mode = Mode::RecurrenceRev,
                "-" => app.factor = app.factor - 0.1,
                "+" => app.factor = app.factor + 0.1,
                "<" => app.exponent = app.exponent - 0.1,
                ">" => app.exponent = app.exponent + 0.1,
                "p" => println!("factor: {}, exponent: {}", app.factor, app.exponent),
                _ => (),
            },
            _ => (),
        }

        match rx.try_recv() {
            Ok(p) => match p.clone() {
                OscPacket::Message(msg) => match msg.addr.as_str() {
                    "/facdown" => {
                        float_osc(p).map(|f| app.down_fac = f);
                        ()
                    }
                    "/facup" => {
                        float_osc(p).map(|f| app.up_fac = f);
                        ()
                    }
                    "/factor" => {
                        float_osc(p).map(|f| app.factor = f);
                        ()
                    }
                    "/exponent" => {
                        float_osc(p).map(|f| app.exponent = f);
                        ()
                    }
                    "/bwmode" => {
                        int_osc(p).map(|f| app.bwmode = f as u8);
                        ()
                    }
                    "/offsetx" => {
                        int_osc(p).map(|f| app.offset_x = f);
                        ()
                    }
                    "/offsety" => {
                        int_osc(p).map(|f| app.offset_y = f);
                        ()
                    }
                    "/zoom" => {
                        float_osc(p).map(|f| app.zoom = f);
                        ()
                    }

                    _ => println!("unkown addr"),
                },
                _ => (),
            },
            _ => (),
        };

        if let Some(args) = e.render_args() {
            app.render(&args);
        }

        if let Some(args) = e.update_args() {
            app.update(
                &args,
                &mut consumer_1,
                &mut consumer_2,
                &mut consumer_3,
                app.down_fac,
                app.up_fac,
            );
        }
    }
}
